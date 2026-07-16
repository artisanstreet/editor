import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import {
	Cause,
	Deferred,
	Effect,
	Exit,
	Fiber,
	FileSystem,
	Layer,
	ManagedRuntime,
	Option,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	HostedGitMutationConflict,
	HostedGitMutationInvariant,
	HostedGitMutationRepository,
	HostedGitMutationRepositoryLive,
	HostedGitMutationUnavailable,
} from "../../modules/backend/src/git-provider/hosted-git-mutation-repository";
import {
	WorkspaceGitCheckoutConflict,
	WorkspaceGitCheckoutRepository,
	WorkspaceGitCheckoutRepositoryLive,
	type RequestWorkspaceGitCheckout,
	type WorkspaceGitCheckoutDecision,
} from "../../modules/backend/src/git/workspace-git-checkout-repository";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import {
	WorkspaceGitMutationConflict,
	WorkspaceGitMutationRepository,
	WorkspaceGitMutationRepositoryLive,
	type RequestWorkspaceGitMutation,
	type WorkspaceGitMutationDecision,
} from "../../modules/backend/src/git/workspace-git-mutation-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	HostedGitMutationApprovals,
	HostedGitMutationArtifacts,
	HostedGitMutationClaims,
	HostedGitSnapshots,
	JournalCommands,
	JournalEvents,
	ProjectHostedOrigins,
	Projects,
	ThreadErasureClaims,
	Threads,
	WorkspaceGitCheckoutApprovals,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitMutationApprovals,
	WorkspaceGitMutationClaims,
	WorkspaceGitSessions,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-16T12:00:00.000Z";
const later = "2026-07-16T12:01:00.000Z";
const body = "This must remain private.";

interface TestClock {
	queued_values: Array<string>;
	value: string;
}

const repository = { host: "github.com", name: "editor", owner: "artisan", provider_id: "github" };
const selection = { account_login: "alice", host: "github.com", provider_id: "github" };
const pull_request_origin = {
	native_id: "PR_42",
	provider_id: "github",
	resource_kind: "pull_request" as const,
};
const review_thread_origin = {
	native_id: "RT_7",
	provider_id: "github",
	resource_kind: "review_thread" as const,
};
const workflow_origin = {
	native_id: "WR_9",
	provider_id: "github",
	resource_kind: "workflow_run" as const,
};
const provider_result = {
	operation: "reply_review_thread" as const,
	origin: {
		native_id: "RC_1",
		provider_id: "github",
		resource_kind: "review_comment" as const,
	},
	status: "applied" as const,
	thread_origin: review_thread_origin,
};

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-hosted-git-mutation-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(
	database_path: string,
	instance_id = "hosted_git_mutation_test",
	clock: TestClock = { queued_values: [], value: now },
) {
	let next_id = 0;
	const infrastructure = Layer.mergeAll(
		NodeCrypto.layer,
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id,
			MakeId: (prefix) => Effect.sync(() => `${prefix}_${instance_id}_${++next_id}`),
			Now: Effect.sync(() => clock.queued_values.shift() ?? clock.value),
		}),
		JournalNotifierLive,
	);
	const repositories = Layer.mergeAll(
		HostedGitMutationRepositoryLive,
		WorkspaceGitCheckoutRepositoryLive,
		WorkspaceGitMutationRepositoryLive,
	).pipe(Layer.provideMerge(infrastructure));

	return ManagedRuntime.make(repositories);
}

async function list_executing_for(
	state: "owned" | "waiting" | "prelaunch" | "quarantine" | "result_recorded",
) {
	const database_path = await Effect.runPromise(MakeDatabasePath);
	const owner = make_runtime(database_path, "dispatch_owner");
	const observer = make_runtime(database_path, "dispatch_observer", {
		queued_values: [],
		value:
			state === "prelaunch" || state === "quarantine" || state === "result_recorded"
				? later
				: now,
	});

	try {
		await owner.runPromise(
			Effect.gen(function* () {
				const mutations = yield* HostedGitMutationRepository;

				yield* Seed;
				yield* mutations.Request(request());
				yield* mutations.Decide({
					approval_id: "approval_1",
					approved: true,
					decision_command: { message_id: "decision_approval_1", sent_at: later },
					thread_id: "thread_1",
				});
				yield* mutations.MarkExecuting("approval_1");
				const execution = yield* mutations.ReadExecution("approval_1");

				if (state === "quarantine" || state === "result_recorded") {
					yield* mutations.ExecuteClaimed(
						{ approval_id: "approval_1", claim_token: execution.claim_token },
						Effect.void,
					);
				}
				if (state === "result_recorded") {
					yield* mutations.RecordProviderResult({
						approval_id: "approval_1",
						claim_token: execution.claim_token,
						result: provider_result,
					});
				}
			}),
		);

		if (state === "prelaunch" || state === "result_recorded") {
			return await observer.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) => mutations.ListExecuting),
			);
		}
		if (state === "quarantine") {
			return await observer.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) => mutations.ListExecuting),
			);
		}
		if (state === "waiting") {
			return await observer.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) => mutations.ListExecuting),
			);
		}

		return await owner.runPromise(
			Effect.flatMap(HostedGitMutationRepository, (mutations) => mutations.ListExecuting),
		);
	} finally {
		await Promise.all([owner.dispose(), observer.dispose()]);
	}
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) return Cause.squash(exit.cause);

	throw new Error("Expected failure");
}

function snapshot_lookup() {
	return {
		association: {
			_tag: "matched" as const,
			freshness: "current" as const,
			pull_request: {
				base_branch: "main",
				base_commit: "b".repeat(40),
				checks: [
					{
						annotations: [],
						annotations_truncated: false,
						name: "CI",
						origin: {
							native_id: "CR_8",
							provider_id: "github",
							resource_kind: "check_run",
						},
						required: true,
						state: "passed" as const,
						workflow_origin,
					},
				],
				checks_total: 1,
				checks_truncated: false,
				draft: false,
				head_branch: "feature",
				head_commit: "a".repeat(40),
				mergeability: "mergeable" as const,
				number: 42,
				origin: pull_request_origin,
				requested_reviewers: [],
				requested_reviewers_truncated: false,
				review_decision: "none" as const,
				review_threads: [
					{
						comment_count: 1,
						origin: review_thread_origin,
						outdated: false,
						path: "src/main.ts",
						resolved: false,
						subject: "line" as const,
					},
				],
				review_threads_total: 1,
				review_threads_truncated: false,
				reviews: [],
				reviews_total: 0,
				reviews_truncated: false,
				state: "open" as const,
				title: "Mutation target",
				web_url: "https://github.com/artisan/editor/pull/42",
			},
		},
		branch: "feature",
		expected_head_commit: "a".repeat(40),
		repository,
	};
}

const Seed = Effect.gen(function* () {
	const database = yield* Database;
	const lookup = snapshot_lookup();

	yield* database.client.insert(Projects).values({
		canonical_root: "C:/project",
		display_name: "Artisan",
		project_id: "project_1",
		registered_at: now,
		updated_at: now,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(ProjectHostedOrigins).values({
		canonical_host: "github.com",
		clone_url: "https://github.com/artisan/editor.git",
		fetch_url: "https://github.com/artisan/editor.git",
		name: "editor",
		native_id: "repository_1",
		owner: "artisan",
		project_id: "project_1",
		provider_id: "github",
		push_url: "https://github.com/artisan/editor.git",
		remote_name: "origin",
		selected_account_login: "alice",
		web_url: "https://github.com/artisan/editor",
	});
	yield* database.client.insert(WorkspaceGitSessions).values({
		additions: 0,
		blockers_json: "[]",
		branch: "feature",
		deletions: 0,
		files: 0,
		has_diff: false,
		head: "a".repeat(40),
		journal_sequence: 1,
		observed_at: now,
		repository_root: "C:/project",
		selected_worktree_path: "C:/project",
		state: "ready",
		updated_at: now,
		version: 1,
		workspace_id: "workspace_1",
	});
	yield* database.client.insert(Threads).values([
		{
			created_at: now,
			primary_project_id: "project_1",
			primary_project_json: JSON.stringify({
				display_name: "Artisan",
				project_id: "project_1",
				root_path: "C:/project",
			}),
			thread_id: "thread_1",
			title: "Attached",
			title_source: "initial",
			updated_at: now,
		},
		{
			created_at: now,
			thread_id: "thread_2",
			title: "Other",
			title_source: "initial",
			updated_at: now,
		},
	]);
	yield* database.client.insert(HostedGitSnapshots).values({
		journal_sequence: 1,
		lookup_json: JSON.stringify(lookup),
		observed_at: now,
		project_id: "project_1",
		version: 1,
	});
});

function request(overrides: Record<string, unknown> = {}) {
	return {
		approval_id: "approval_1",
		command: {
			mutation: {
				body,
				expected_head_commit: "a".repeat(40),
				operation: "reply_review_thread",
				pull_request_number: 42,
				pull_request_origin,
				repository,
				selected_branch: "feature",
				snapshot_version: 1,
				thread_origin: review_thread_origin,
				workspace_id: "workspace_1",
			},
			selection,
		},
		request_fingerprint: "f".repeat(64),
		source_command: { message_id: "request_1", sent_at: now },
		thread_id: "thread_1",
		...overrides,
	};
}

function identified_request(identifier: string, overrides: Record<string, unknown> = {}) {
	return request({
		approval_id: `approval_${identifier}`,
		source_command: { message_id: `request_${identifier}`, sent_at: now },
		...overrides,
	});
}

function workspace_checkout_request(): RequestWorkspaceGitCheckout {
	return {
		approval_id: "checkout_approval_1",
		expected_session_version: 1,
		request_fingerprint: "c".repeat(64),
		source_command: { message_id: "checkout_request_1", sent_at: now },
		target_branch: "release",
		target_head: "c".repeat(40),
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function workspace_checkout_decision(): WorkspaceGitCheckoutDecision {
	return {
		approval_id: "checkout_approval_1",
		approved: true,
		decision_command: { message_id: "checkout_decision_1", sent_at: later },
		thread_id: "thread_1",
	};
}

function workspace_git_source_proof() {
	const identity = "d".repeat(64);

	return {
		branch: "feature",
		configuration_identity: identity,
		head: "a".repeat(40),
		index_identity: identity,
		repository_identity: identity,
		state: "none" as const,
		state_identity: identity,
		status_identity: identity,
		tracked_identity: identity,
		untracked_identity: identity,
		worktree_identity: identity,
	};
}

function workspace_mutation_request(): RequestWorkspaceGitMutation {
	const source = workspace_git_source_proof();
	const operation = { message: "Private commit message", type: "commit" as const };

	return {
		approval_id: "mutation_approval_1",
		expected_session_version: 1,
		operation,
		plan: {
			binding: "d".repeat(64),
			message: operation.message,
			source,
			type: "commit",
		},
		request_fingerprint: "d".repeat(64),
		source_command: { message_id: "mutation_request_1", sent_at: now },
		thread_id: "thread_1",
		workspace_id: "workspace_1",
	};
}

function workspace_mutation_decision(): WorkspaceGitMutationDecision {
	return {
		approval_id: "mutation_approval_1",
		approved: true,
		decision_command: { message_id: "mutation_decision_1", sent_at: later },
		thread_id: "thread_1",
	};
}

type TestRuntime = ReturnType<typeof make_runtime>;

type RunLocalClaim = (
	runtime: TestRuntime,
	start: Deferred.Deferred<void>,
	ready: Deferred.Deferred<void>,
) => Promise<Exit.Exit<unknown, unknown>>;

async function race_workspace_claims(
	hosted_runtime: TestRuntime,
	local_runtime: TestRuntime,
	RunLocalClaim: RunLocalClaim,
) {
	const [start, hosted_ready, local_ready] = await Effect.runPromise(
		Effect.all([Deferred.make<void>(), Deferred.make<void>(), Deferred.make<void>()]),
	);
	const hosted_claim = hosted_runtime.runPromise(
		Effect.gen(function* () {
			const hosted = yield* HostedGitMutationRepository;

			yield* Deferred.succeed(hosted_ready, undefined);
			yield* Deferred.await(start);

			return yield* Effect.exit(hosted.MarkExecuting("approval_1"));
		}),
	);
	const local_claim = RunLocalClaim(local_runtime, start, local_ready);

	await Effect.runPromise(
		Effect.all([Deferred.await(hosted_ready), Deferred.await(local_ready)], {
			discard: true,
		}).pipe(Effect.andThen(Deferred.succeed(start, undefined))),
	);

	const [hosted_exit, local_exit] = await Promise.all([hosted_claim, local_claim]);
	const claim_count = await hosted_runtime.runPromise(
		Effect.gen(function* () {
			const database = yield* Database;
			const hosted = yield* database.client.select().from(HostedGitMutationClaims);
			const checkout = yield* database.client.select().from(WorkspaceGitCheckoutClaims);
			const mutation = yield* database.client.select().from(WorkspaceGitMutationClaims);

			return hosted.length + checkout.length + mutation.length;
		}),
	);

	return { claim_count, hosted_exit, local_exit };
}

afterEach(async () => {
	const directories = temporary_directories.splice(0);
	await Effect.runPromise(
		Effect.forEach(
			directories,
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("HostedGitMutationRepository", () => {
	it("journals an exact request once, redacts its reply, and survives a fresh runtime", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					yield* Seed;
					const accepted = yield* mutations.Request(request());
					const duplicate = yield* mutations.Request(request());

					return {
						accepted,
						approvals: yield* database.client.select().from(HostedGitMutationApprovals),
						artifacts: yield* database.client.select().from(HostedGitMutationArtifacts),
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);

			expect(result.duplicate).toEqual({ ...result.accepted, status: "duplicate" });
			expect(result.accepted.event).toMatchObject({
				causation_id: "request_1",
				correlation_id: "approval_1",
				kind: "event",
				payload: {
					approval: { approval_id: "approval_1", state: "requested" },
					type: "hosted.git.mutation.approval.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				stream_id: "thread:thread_1",
				thread_id: "thread_1",
			});
			expect(
				JSON.stringify([result.approvals, result.commands, result.events]),
			).not.toContain(body);
			expect(result.approvals[0]!.selection_json).toBe(JSON.stringify(selection));
			expect(result.artifacts[0]!.operation_json).toContain(body);
			await runtime.dispose();

			const fresh_runtime = make_runtime(database_path);
			try {
				const replay = await fresh_runtime.runPromise(
					Effect.flatMap(HostedGitMutationRepository, (mutations) =>
						mutations.ReplayRequest(request()),
					),
				);
				expect(replay).toMatchObject({
					value: { approval: { state: "requested" }, status: "duplicate" },
				});
			} finally {
				await fresh_runtime.dispose();
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("converges exact requests and rejects competing intent across two runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const left_runtime = make_runtime(database_path, "left");
		const right_runtime = make_runtime(database_path, "right");

		try {
			await left_runtime.runPromise(Seed);
			const ExactRequest = (runtime: typeof left_runtime) =>
				runtime.runPromise(
					Effect.flatMap(HostedGitMutationRepository, (mutations) =>
						mutations.Request(identified_request("exact_race")),
					),
				);
			const exact = await Promise.all([
				ExactRequest(left_runtime),
				ExactRequest(right_runtime),
			]);
			expect(exact.map((acceptance) => acceptance.status).sort()).toEqual([
				"accepted",
				"duplicate",
			]);

			const competing = identified_request("intent_race");
			const changed = {
				...competing,
				command: {
					...competing.command,
					mutation: {
						...competing.command.mutation,
						body: "Competing private intent.",
					},
				},
				request_fingerprint: "e".repeat(64),
			};
			const CompetingRequest = (
				runtime: typeof left_runtime,
				input: ReturnType<typeof identified_request>,
			) =>
				runtime.runPromise(
					Effect.flatMap(HostedGitMutationRepository, (mutations) =>
						Effect.exit(mutations.Request(input)),
					),
				);
			const race = await Promise.all([
				CompetingRequest(left_runtime, competing),
				CompetingRequest(right_runtime, changed),
			]);
			const successful = race.filter(Exit.isSuccess);
			const failed = race.filter(Exit.isFailure);
			expect(successful).toHaveLength(1);
			expect(failed).toHaveLength(1);
			expect(failure_from(failed[0]!)).toEqual(
				new HostedGitMutationConflict({ reason: "request_conflict" }),
			);

			const stored = await left_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					return {
						approvals: yield* database.client.select().from(HostedGitMutationApprovals),
						artifacts: yield* database.client.select().from(HostedGitMutationArtifacts),
						commands: yield* database.client.select().from(JournalCommands),
						events: yield* database.client.select().from(JournalEvents),
					};
				}),
			);
			expect(stored.approvals).toHaveLength(2);
			expect(stored.artifacts).toHaveLength(2);
			expect(stored.commands).toHaveLength(2);
			expect(stored.events).toHaveLength(2);
		} finally {
			await Promise.all([left_runtime.dispose(), right_runtime.dispose()]);
		}
	});

	it("fails closed for changed intent, cross-thread reads, and corrupt durable bindings", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					yield* Seed;
					yield* mutations.Request(request());
					const [approval] = yield* database.client
						.select()
						.from(HostedGitMutationApprovals);
					const [artifact] = yield* database.client
						.select()
						.from(HostedGitMutationArtifacts);
					const [event] = yield* database.client.select().from(JournalEvents);
					if (!approval || !artifact || !event) {
						return yield* Effect.die("Expected hosted mutation state");
					}
					const changed = yield* Effect.exit(
						mutations.Request(request({ request_fingerprint: "e".repeat(64) })),
					);
					const cross_thread = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_2" }),
					);
					yield* database.client
						.update(HostedGitMutationArtifacts)
						.set({ operation_binding: "0".repeat(64) });
					const corrupt_binding = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client
						.update(HostedGitMutationArtifacts)
						.set({ operation_binding: artifact.operation_binding });
					yield* database.client
						.update(HostedGitMutationArtifacts)
						.set({ operation_json: JSON.stringify({ invalid: true }) });
					const corrupt_private_operation = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client
						.update(HostedGitMutationArtifacts)
						.set({ operation_json: artifact.operation_json });
					yield* database.client.update(HostedGitMutationArtifacts).set({
						selection_json: JSON.stringify({
							...selection,
							account_login: "mallory",
						}),
					});
					const corrupt_private_selection = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client
						.update(HostedGitMutationArtifacts)
						.set({ selection_json: artifact.selection_json });
					yield* database.client
						.update(HostedGitMutationApprovals)
						.set({ operation_summary_json: JSON.stringify({ invalid: true }) });
					const corrupt_public_summary = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client
						.update(HostedGitMutationApprovals)
						.set({ operation_summary_json: approval.operation_summary_json });
					yield* database.client.update(JournalEvents).set({ origin: "frontend" });
					const corrupt_event = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client.update(JournalEvents).set({ origin: event.origin });
					yield* database.client
						.update(JournalEvents)
						.set({ event_type: "hosted.git.snapshot.updated" });
					const corrupt_event_type = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client
						.update(JournalEvents)
						.set({ event_type: event.event_type });
					yield* database.client.update(JournalEvents).set({
						raw_origin_json: JSON.stringify({
							provider: "engine_test",
							reference: "native-thread-agent",
						}),
					});
					const corrupt_event_raw_origin = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client.update(JournalEvents).set({ raw_origin_json: null });
					yield* database.client.update(JournalEvents).set({ run_id: "run_rogue" });
					const corrupt_event_run = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client.update(JournalEvents).set({ run_id: null });
					yield* database.client.update(JournalEvents).set({ agent_id: "agent_rogue" });
					const corrupt_event_agent = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);
					yield* database.client.update(JournalEvents).set({ agent_id: null });

					return {
						changed,
						corrupt_binding,
						corrupt_event,
						corrupt_event_agent,
						corrupt_event_raw_origin,
						corrupt_event_run,
						corrupt_event_type,
						corrupt_private_operation,
						corrupt_private_selection,
						corrupt_public_summary,
						cross_thread,
					};
				}),
			);

			expect(failure_from(result.changed)).toBeInstanceOf(HostedGitMutationConflict);
			expect(failure_from(result.cross_thread)).toEqual(
				new HostedGitMutationUnavailable({ reason: "missing" }),
			);
			for (const corrupt of [
				result.corrupt_binding,
				result.corrupt_event,
				result.corrupt_event_agent,
				result.corrupt_event_raw_origin,
				result.corrupt_event_run,
				result.corrupt_event_type,
				result.corrupt_private_operation,
				result.corrupt_private_selection,
				result.corrupt_public_summary,
			]) {
				const failure = failure_from(corrupt);

				expect(failure).toEqual(
					new HostedGitMutationInvariant({
						message: "Stored hosted Git mutation state is invalid",
					}),
				);
				expect(JSON.stringify(failure)).not.toContain(body);
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects stale local checkout state and inconsistent exact-head pull requests", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					const Attempt = (identifier: string) =>
						Effect.exit(mutations.Request(identified_request(identifier)));
					yield* Seed;

					yield* database.client
						.update(WorkspaceGitSessions)
						.set({ head: "b".repeat(40) });
					const stale_checkout = yield* Attempt("stale_checkout");
					yield* database.client
						.update(WorkspaceGitSessions)
						.set({ head: "a".repeat(40) });

					yield* database.client.update(HostedGitSnapshots).set({ version: 2 });
					const stale_snapshot = yield* Attempt("stale_snapshot");
					yield* database.client.update(HostedGitSnapshots).set({ version: 1 });

					const wrong_head = snapshot_lookup();
					wrong_head.association.pull_request.head_commit = "b".repeat(40);
					yield* database.client
						.update(HostedGitSnapshots)
						.set({ lookup_json: JSON.stringify(wrong_head) });
					const inconsistent_head = yield* Attempt("inconsistent_head");

					const current = snapshot_lookup();
					const closed = {
						...current,
						association: {
							...current.association,
							pull_request: {
								...current.association.pull_request,
								state: "closed" as const,
							},
						},
					};
					yield* database.client
						.update(HostedGitSnapshots)
						.set({ lookup_json: JSON.stringify(closed) });
					const closed_pull_request = yield* Attempt("closed_pull_request");

					const wrong_pull_request = identified_request("wrong_pull_request");
					const mismatched_pull_request = yield* Effect.exit(
						mutations.Request({
							...wrong_pull_request,
							command: {
								...wrong_pull_request.command,
								mutation: {
									...wrong_pull_request.command.mutation,
									pull_request_number: 43,
								},
							},
						}),
					);
					const wrong_thread = identified_request("wrong_thread");
					const mismatched_review_thread = yield* Effect.exit(
						mutations.Request({
							...wrong_thread,
							command: {
								...wrong_thread.command,
								mutation: {
									...wrong_thread.command.mutation,
									thread_origin: {
										...review_thread_origin,
										native_id: "RT_other",
									},
								},
							},
						}),
					);
					const approvals = yield* database.client
						.select()
						.from(HostedGitMutationApprovals);

					return {
						approvals,
						closed_pull_request,
						inconsistent_head,
						mismatched_pull_request,
						mismatched_review_thread,
						stale_checkout,
						stale_snapshot,
					};
				}),
			);

			for (const rejected of [
				result.stale_checkout,
				result.stale_snapshot,
				result.inconsistent_head,
				result.closed_pull_request,
				result.mismatched_pull_request,
				result.mismatched_review_thread,
			]) {
				expect(failure_from(rejected)).toEqual(
					new HostedGitMutationConflict({ reason: "request_conflict" }),
				);
			}
			expect(result.approvals).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("requires the selected provider account and an intact visible project attachment", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					const Attempt = (identifier: string) =>
						Effect.exit(mutations.Request(identified_request(identifier)));
					const project_reference = JSON.stringify({
						display_name: "Artisan",
						project_id: "project_1",
						root_path: "C:/project",
					});
					yield* Seed;

					yield* database.client
						.update(ProjectHostedOrigins)
						.set({ selected_account_login: "mallory" });
					const wrong_account = yield* Attempt("wrong_account");
					yield* database.client
						.update(ProjectHostedOrigins)
						.set({ selected_account_login: "alice" });

					yield* database.client
						.update(ProjectHostedOrigins)
						.set({ owner: "someone-else" });
					const wrong_repository = yield* Attempt("wrong_repository");
					yield* database.client.update(ProjectHostedOrigins).set({ owner: "artisan" });

					yield* database.client
						.update(Threads)
						.set({ primary_project_id: null, primary_project_json: null });
					const unattached = yield* Attempt("unattached");
					yield* database.client.update(Threads).set({
						primary_project_id: "project_1",
						primary_project_json: JSON.stringify({
							display_name: "Artisan",
							project_id: "project_1",
							root_path: "C:/tampered",
						}),
					});
					const corrupt_attachment = yield* Attempt("corrupt_attachment");

					yield* database.client.update(Threads).set({
						primary_project_id: null,
						primary_project_json: project_reference,
					});
					const corrupt_primary_pair = yield* Attempt("corrupt_primary_pair");
					yield* database.client.update(Threads).set({
						primary_project_id: "project_1",
						primary_project_json: project_reference,
					});

					yield* database.client
						.update(WorkspaceGitSessions)
						.set({ selected_worktree_path: "C:/hidden-worktree" });
					const corrupt_visible_checkout = yield* Attempt("corrupt_visible_checkout");
					yield* database.client.delete(WorkspaceGitSessions);
					const missing_session = yield* Attempt("missing_session");
					const approvals = yield* database.client
						.select()
						.from(HostedGitMutationApprovals);

					return {
						approvals,
						corrupt_attachment,
						corrupt_primary_pair,
						corrupt_visible_checkout,
						missing_session,
						unattached,
						wrong_account,
						wrong_repository,
					};
				}),
			);

			for (const rejected of [result.wrong_account, result.wrong_repository]) {
				expect(failure_from(rejected)).toEqual(
					new HostedGitMutationConflict({ reason: "request_conflict" }),
				);
			}
			expect(failure_from(result.unattached)).toEqual(
				new HostedGitMutationUnavailable({ reason: "thread_not_attached" }),
			);
			for (const corrupt of [
				result.corrupt_attachment,
				result.corrupt_primary_pair,
				result.corrupt_visible_checkout,
			]) {
				expect(failure_from(corrupt)).toEqual(
					new HostedGitMutationInvariant({
						message: "Stored hosted Git mutation state is invalid",
					}),
				);
			}
			expect(failure_from(result.missing_session)).toEqual(
				new HostedGitMutationConflict({ reason: "request_conflict" }),
			);
			expect(result.approvals).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("blocks a first decision once thread erasure starts and still replays a prior decision", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));
		const decision = {
			approval_id: "approval_1",
			approved: true,
			decision_command: { message_id: "decision_1", sent_at: later },
			thread_id: "thread_1",
		};

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					yield* Seed;
					yield* mutations.Request(request());
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: later,
						thread_id: "thread_1",
					});
					const blocked = yield* Effect.exit(mutations.Decide(decision));
					const requested = yield* mutations.Query({
						approval_id: "approval_1",
						thread_id: "thread_1",
					});
					yield* database.client.delete(ThreadErasureClaims);
					const approved = yield* mutations.Decide(decision);
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: later,
						thread_id: "thread_1",
					});
					const duplicate = yield* mutations.Decide(decision);

					return { approved, blocked, duplicate, requested };
				}),
			);

			expect(failure_from(result.blocked)).toEqual(
				new HostedGitMutationUnavailable({ reason: "erased" }),
			);
			expect(result.requested.approval.state).toBe("requested");
			expect(result.approved.approval.state).toBe("approved");
			expect(result.duplicate).toEqual({
				...result.approved,
				status: "duplicate",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("records approve and deny decisions once and rejects changed or cross-thread decisions", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					const denied_request = request({
						approval_id: "approval_2",
						source_command: { message_id: "request_2", sent_at: later },
					});
					yield* Seed;
					yield* mutations.Request(request());
					const approved = yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_1", sent_at: later },
						thread_id: "thread_1",
					});
					const duplicate = yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_1", sent_at: later },
						thread_id: "thread_1",
					});
					const changed = yield* Effect.exit(
						mutations.Decide({
							approval_id: "approval_1",
							approved: false,
							decision_command: { message_id: "decision_1", sent_at: later },
							thread_id: "thread_1",
						}),
					);
					const cross_thread = yield* Effect.exit(
						mutations.Decide({
							approval_id: "approval_1",
							approved: true,
							decision_command: { message_id: "decision_2", sent_at: later },
							thread_id: "thread_2",
						}),
					);

					yield* mutations.Request(denied_request);
					const denied = yield* mutations.Decide({
						approval_id: "approval_2",
						approved: false,
						decision_command: { message_id: "decision_2", sent_at: later },
						thread_id: "thread_1",
					});
					const replayed_denial = yield* mutations.ReplayRequest(denied_request);
					const duplicate_denial = yield* mutations.Request(denied_request);
					const changed_denial = yield* Effect.exit(
						mutations.Request({
							...denied_request,
							command: {
								...denied_request.command,
								mutation: {
									...denied_request.command.mutation,
									body: "A different private reply.",
								},
							},
						}),
					);
					const queried_denial = yield* mutations.Query({
						approval_id: "approval_2",
						thread_id: "thread_1",
					});
					const by_source = yield* mutations.ReadBySourceCommand("request_2");
					const artifacts = yield* database.client
						.select()
						.from(HostedGitMutationArtifacts);
					const durable_state = yield* Effect.all({
						approvals: database.client.select().from(HostedGitMutationApprovals),
						commands: database.client.select().from(JournalCommands),
						events: database.client.select().from(JournalEvents),
					});

					return {
						approved,
						artifacts,
						by_source,
						changed,
						changed_denial,
						cross_thread,
						denied,
						duplicate,
						duplicate_denial,
						durable_state,
						queried_denial,
						replayed_denial,
					};
				}),
			);

			expect(result.approved.approval.state).toBe("approved");
			expect(result.denied.approval.state).toBe("denied");
			expect(result.duplicate).toEqual({ ...result.approved, status: "duplicate" });
			expect(result.duplicate_denial).toEqual({
				...result.denied,
				status: "duplicate",
			});
			expect(result.replayed_denial).toMatchObject({
				_tag: "Some",
				value: { approval: { state: "denied" }, status: "duplicate" },
			});
			expect(result.by_source).toMatchObject({
				_tag: "Some",
				value: { approval: { state: "denied" }, status: "duplicate" },
			});
			expect(result.queried_denial.approval.state).toBe("denied");
			const denied_artifact = result.artifacts.find(
				(artifact) => artifact.approval_id === "approval_2",
			);
			expect(denied_artifact).toMatchObject({
				operation_json: null,
				provider_result_json: null,
				selection_json: null,
			});
			expect(denied_artifact?.operation_binding).toMatch(/^[a-f0-9]{64}$/u);
			expect(JSON.stringify(result.durable_state)).not.toContain(body);
			expect(failure_from(result.changed)).toBeInstanceOf(HostedGitMutationConflict);
			expect(failure_from(result.changed_denial)).toBeInstanceOf(HostedGitMutationConflict);
			expect(failure_from(result.cross_thread)).toEqual(
				new HostedGitMutationUnavailable({ reason: "missing" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("durably claims and launches one hosted mutation without changing its public executing event", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					const listed = yield* mutations.ListApproved;
					const executing = yield* mutations.MarkExecuting("approval_1");
					const before_launch = yield* Effect.all({
						approval: database.client.select().from(HostedGitMutationApprovals),
						artifact: database.client.select().from(HostedGitMutationArtifacts),
						claim: database.client.select().from(HostedGitMutationClaims),
						events: database.client.select().from(JournalEvents),
					});
					const execution = yield* mutations.ReadExecution("approval_1");
					const observed = yield* mutations.ExecuteClaimed(
						{ approval_id: "approval_1", claim_token: execution.claim_token },
						Effect.gen(function* () {
							const query = yield* mutations.Query({
								approval_id: "approval_1",
								thread_id: "thread_1",
							});
							const [approval] = yield* database.client
								.select()
								.from(HostedGitMutationApprovals);
							const [claim] = yield* database.client
								.select()
								.from(HostedGitMutationClaims);

							return { approval, claim, query };
						}),
					);
					const after_launch = yield* Effect.all({
						approval: database.client.select().from(HostedGitMutationApprovals),
						artifact: database.client.select().from(HostedGitMutationArtifacts),
						claim: database.client.select().from(HostedGitMutationClaims),
						events: database.client.select().from(JournalEvents),
					});

					return { after_launch, before_launch, executing, execution, listed, observed };
				}),
			);

			expect(result.listed).toEqual([{ approval_id: "approval_1", thread_id: "thread_1" }]);
			expect(result.executing.approval).toMatchObject({
				state: "executing",
				updated_at: now,
			});
			expect(result.execution.command).toMatchObject({ mutation: { body } });
			expect(result.before_launch.approval[0]).toMatchObject({
				state: "executing",
				execution_started_at: null,
				updated_at: now,
			});
			expect(result.before_launch.artifact[0]).toMatchObject({
				provider_result_json: null,
				updated_at: now,
			});
			expect(result.before_launch.claim[0]).toMatchObject({
				execution_completed_at: null,
				execution_started_at: null,
				owner_instance_id: "hosted_git_mutation_test",
			});
			expect(result.observed.query.approval).toEqual(result.executing.approval);
			expect(result.observed.approval?.execution_started_at).toBe(
				result.observed.claim?.execution_started_at,
			);
			expect(result.observed.claim?.execution_started_at).toBe(now);
			expect(result.after_launch.approval[0]).toMatchObject({
				execution_started_at: now,
				updated_at: now,
			});
			expect(result.after_launch.artifact[0]).toMatchObject({ updated_at: now });
			expect(result.after_launch.claim[0]).toMatchObject({
				execution_completed_at: now,
				execution_started_at: now,
			});
			expect(result.after_launch.events).toHaveLength(3);
			expect(result.before_launch.events).toHaveLength(3);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects foreign, expired, and competing workspace claims before provider launch", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const owner_clock: TestClock = { queued_values: [], value: now };
		const owner_runtime = make_runtime(database_path, "owner", owner_clock);
		const foreign_runtime = make_runtime(database_path, "foreign");

		try {
			const result = await owner_runtime.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					const wrong_token = yield* Effect.exit(
						mutations.RenewLease({
							approval_id: "approval_1",
							claim_token: "claim_wrong",
						}),
					);
					owner_clock.value = later;
					const expired = yield* Effect.exit(
						mutations.RenewLease({
							approval_id: "approval_1",
							claim_token: execution.claim_token,
						}),
					);

					return { execution, expired, wrong_token };
				}),
			);
			let foreign_launches = 0;
			const foreign = await foreign_runtime.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;

					return yield* Effect.exit(
						mutations.ExecuteClaimed(
							{
								approval_id: "approval_1",
								claim_token: result.execution.claim_token,
							},
							Effect.sync(() => {
								foreign_launches += 1;
							}),
						),
					);
				}),
			);

			expect(failure_from(result.wrong_token)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(failure_from(result.expired)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(failure_from(foreign)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(foreign_launches).toBe(0);
		} finally {
			await Promise.all([owner_runtime.dispose(), foreign_runtime.dispose()]);
		}
	});

	it("revalidates dispatcher and journal authority immediately before provider launch", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					let launches = 0;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* database.client.run(`
						UPDATE journal_events
						SET origin = 'frontend'
						WHERE idempotency_key = 'hosted_git_mutation:approval_1:approved'
					`);
					const corrupt_dispatch = yield* Effect.exit(mutations.ListApproved);
					yield* database.client.run(`
						UPDATE journal_events
						SET origin = 'backend'
						WHERE idempotency_key = 'hosted_git_mutation:approval_1:approved'
					`);
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					yield* database.client.run(`
						CREATE TRIGGER corrupt_hosted_mutation_launch_authority
						AFTER UPDATE OF lease_expires_at ON hosted_git_mutation_claims
						WHEN NEW.approval_id = 'approval_1'
						BEGIN
							UPDATE journal_events
							SET payload_json = '{}'
							WHERE idempotency_key = 'hosted_git_mutation:approval_1:executing';
						END
					`);
					const launch = yield* Effect.exit(
						mutations.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: execution.claim_token },
							Effect.sync(() => {
								launches += 1;
							}),
						),
					);
					const [claim] = yield* database.client.select().from(HostedGitMutationClaims);

					return { claim, corrupt_dispatch, launch, launches };
				}),
			);
			const expected = new HostedGitMutationInvariant({
				message: "Stored hosted Git mutation state is invalid",
			});

			expect(failure_from(result.corrupt_dispatch)).toEqual(expected);
			expect(failure_from(result.launch)).toEqual(expected);
			expect(result.claim?.execution_started_at).toBeNull();
			expect(result.launches).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("refuses launch when a renewed lease expires before the start transaction", async () => {
		const clock: TestClock = { queued_values: [], value: now };
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"lease_boundary",
			clock,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					let launches = 0;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					clock.queued_values.push(
						"2026-07-16T12:00:29.000Z",
						"2026-07-16T12:01:00.000Z",
					);
					const launch = yield* Effect.exit(
						mutations.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: execution.claim_token },
							Effect.sync(() => {
								launches += 1;
							}),
						),
					);
					const [claim] = yield* database.client.select().from(HostedGitMutationClaims);
					yield* database.client.delete(HostedGitMutationClaims);
					const orphaned = yield* Effect.exit(
						mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
					);

					return { claim, launch, launches, orphaned };
				}),
			);

			expect(failure_from(result.launch)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(failure_from(result.orphaned)).toEqual(
				new HostedGitMutationInvariant({
					message: "Stored hosted Git mutation state is invalid",
				}),
			);
			expect(result.claim).toMatchObject({
				execution_started_at: null,
				lease_expires_at: "2026-07-16T12:00:59.000Z",
			});
			expect(result.launches).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("refuses checkout and local mutation workspace claims before hosted admission", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* database.client.insert(WorkspaceGitCheckoutApprovals).values({
						approval_id: "checkout_1",
						created_at: now,
						expected_session_version: 1,
						request_fingerprint: "c".repeat(64),
						source_branch: "main",
						source_command_id: "checkout_request_1",
						source_head: "a".repeat(40),
						state: "requested",
						target_branch: "feature",
						target_head: "a".repeat(40),
						thread_id: "thread_1",
						updated_at: now,
						workspace_id: "workspace_1",
					});
					yield* database.client.insert(WorkspaceGitCheckoutClaims).values({
						approval_id: "checkout_1",
						claimed_at: now,
						thread_id: "thread_1",
						workspace_id: "workspace_1",
					});
					const checkout = yield* Effect.exit(mutations.MarkExecuting("approval_1"));
					yield* database.client.delete(WorkspaceGitCheckoutClaims);
					yield* database.client.delete(WorkspaceGitCheckoutApprovals);
					yield* database.client.insert(WorkspaceGitMutationApprovals).values({
						approval_id: "mutation_1",
						created_at: now,
						expected_session_version: 1,
						operation_summary_json: "{}",
						request_fingerprint: "d".repeat(64),
						source_command_id: "mutation_request_1",
						source_head: "a".repeat(40),
						state: "requested",
						thread_id: "thread_1",
						updated_at: now,
						workspace_id: "workspace_1",
					});
					yield* database.client.insert(WorkspaceGitMutationClaims).values({
						approval_id: "mutation_1",
						claim_token: "claim_mutation_1",
						claimed_at: now,
						lease_expires_at: later,
						owner_instance_id: "mutation_owner",
						thread_id: "thread_1",
						workspace_id: "workspace_1",
					});
					const mutation = yield* Effect.exit(mutations.MarkExecuting("approval_1"));

					return { checkout, mutation };
				}),
			);

			expect(failure_from(result.checkout)).toEqual(
				new HostedGitMutationConflict({ reason: "claim_conflict" }),
			);
			expect(failure_from(result.mutation)).toEqual(
				new HostedGitMutationConflict({ reason: "claim_conflict" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("arbitrates simultaneous hosted mutation and checkout claims across two runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const hosted_runtime = make_runtime(database_path, "hosted_checkout_race");
		const checkout_runtime = make_runtime(database_path, "checkout_hosted_race");

		try {
			await hosted_runtime.runPromise(
				Effect.gen(function* () {
					const hosted = yield* HostedGitMutationRepository;
					const checkout = yield* WorkspaceGitCheckoutRepository;

					yield* Seed;
					yield* hosted.Request(request());
					yield* hosted.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* checkout.Request(workspace_checkout_request());
					yield* checkout.Decide(workspace_checkout_decision());
				}),
			);

			const result = await race_workspace_claims(
				hosted_runtime,
				checkout_runtime,
				(runtime, start, ready) =>
					runtime.runPromise(
						Effect.gen(function* () {
							const checkout = yield* WorkspaceGitCheckoutRepository;

							yield* Deferred.succeed(ready, undefined);
							yield* Deferred.await(start);

							return yield* Effect.exit(
								checkout.MarkExecuting("checkout_approval_1"),
							);
						}),
					),
			);

			expect(
				Number(Exit.isSuccess(result.hosted_exit)) +
					Number(Exit.isSuccess(result.local_exit)),
			).toBe(1);
			if (Exit.isFailure(result.hosted_exit)) {
				expect(failure_from(result.hosted_exit)).toEqual(
					new HostedGitMutationConflict({ reason: "claim_conflict" }),
				);
			}
			if (Exit.isFailure(result.local_exit)) {
				expect(failure_from(result.local_exit)).toEqual(
					new WorkspaceGitCheckoutConflict({ reason: "claim_conflict" }),
				);
			}
			expect(result.claim_count).toBe(1);
		} finally {
			await hosted_runtime.dispose();
			await checkout_runtime.dispose();
		}
	});

	it("arbitrates simultaneous hosted and local mutation claims across two runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const hosted_runtime = make_runtime(database_path, "hosted_mutation_race");
		const mutation_runtime = make_runtime(database_path, "mutation_hosted_race");

		try {
			await hosted_runtime.runPromise(
				Effect.gen(function* () {
					const hosted = yield* HostedGitMutationRepository;
					const mutation = yield* WorkspaceGitMutationRepository;

					yield* Seed;
					yield* hosted.Request(request());
					yield* hosted.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutation.Request(workspace_mutation_request());
					yield* mutation.Decide(workspace_mutation_decision());
				}),
			);

			const result = await race_workspace_claims(
				hosted_runtime,
				mutation_runtime,
				(runtime, start, ready) =>
					runtime.runPromise(
						Effect.gen(function* () {
							const mutation = yield* WorkspaceGitMutationRepository;

							yield* Deferred.succeed(ready, undefined);
							yield* Deferred.await(start);

							return yield* Effect.exit(
								mutation.MarkExecuting("mutation_approval_1"),
							);
						}),
					),
			);

			expect(
				Number(Exit.isSuccess(result.hosted_exit)) +
					Number(Exit.isSuccess(result.local_exit)),
			).toBe(1);
			if (Exit.isFailure(result.hosted_exit)) {
				expect(failure_from(result.hosted_exit)).toEqual(
					new HostedGitMutationConflict({ reason: "claim_conflict" }),
				);
			}
			if (Exit.isFailure(result.local_exit)) {
				expect(failure_from(result.local_exit)).toEqual(
					new WorkspaceGitMutationConflict({ reason: "claim_conflict" }),
				);
			}
			expect(result.claim_count).toBe(1);
		} finally {
			await hosted_runtime.dispose();
			await mutation_runtime.dispose();
		}
	});

	it("leaves an interrupted launch quarantined and validates durable active claims", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					const started = yield* Deferred.make<void>();
					let launches = 0;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					const fiber = yield* mutations
						.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: execution.claim_token },
							Effect.gen(function* () {
								launches += 1;
								yield* Deferred.succeed(started, undefined);

								return yield* Effect.never;
							}),
						)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Deferred.await(started);
					yield* Fiber.interrupt(fiber);
					const active = yield* mutations.ActiveClaimsForThread("thread_1");
					const [claim] = yield* database.client.select().from(HostedGitMutationClaims);
					const replay = yield* Effect.exit(
						mutations.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: execution.claim_token },
							Effect.sync(() => {
								launches += 1;
							}),
						),
					);
					yield* database.client
						.update(HostedGitMutationClaims)
						.set({ thread_id: "thread_2" });
					const original_corrupt = yield* Effect.exit(
						mutations.ActiveClaimsForThread("thread_1"),
					);
					const corrupt = yield* Effect.exit(mutations.ActiveClaimsForThread("thread_2"));

					return { active, claim, corrupt, launches, original_corrupt, replay };
				}),
			);

			expect(result.active).toBe(true);
			expect(result.claim).toMatchObject({
				execution_completed_at: null,
				execution_started_at: now,
			});
			expect(failure_from(result.replay)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(result.launches).toBe(1);
			expect(failure_from(result.corrupt)).toEqual(
				new HostedGitMutationInvariant({
					message: "Stored hosted Git mutation state is invalid",
				}),
			);
			expect(failure_from(result.original_corrupt)).toEqual(
				new HostedGitMutationInvariant({
					message: "Stored hosted Git mutation state is invalid",
				}),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("blocks a first hosted claim once thread erasure begins", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const blocked = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* database.client.insert(ThreadErasureClaims).values({
						claimed_at: later,
						thread_id: "thread_1",
					});

					return yield* Effect.exit(mutations.MarkExecuting("approval_1"));
				}),
			);

			expect(failure_from(blocked)).toEqual(
				new HostedGitMutationUnavailable({ reason: "erased" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("records one normalized provider result exactly and rejects mismatched operation or origin", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					const identity = {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};

					yield* mutations.ExecuteClaimed(identity, Effect.void);
					const wrong_operation = yield* mutations
						.RecordProviderResult({
							...identity,
							result: {
								operation: "resolve_review_thread",
								origin: review_thread_origin,
								status: "applied",
							},
						})
						.pipe(Effect.exit);
					const wrong_origin = yield* mutations
						.RecordProviderResult({
							...identity,
							result: {
								...provider_result,
								thread_origin: {
									...review_thread_origin,
									native_id: "RT_other",
								},
							},
						})
						.pipe(Effect.exit);
					yield* database.client.update(JournalEvents).set({ origin: "frontend" });
					const corrupt_journal = yield* mutations
						.RecordProviderResult({ ...identity, result: provider_result })
						.pipe(Effect.exit);
					const [before_valid_result] = yield* database.client
						.select()
						.from(HostedGitMutationArtifacts);
					yield* database.client.update(JournalEvents).set({ origin: "backend" });
					yield* mutations.RecordProviderResult({ ...identity, result: provider_result });
					yield* mutations.RecordProviderResult({ ...identity, result: provider_result });
					const competing = yield* mutations
						.RecordProviderResult({
							...identity,
							result: {
								...provider_result,
								origin: { ...provider_result.origin, native_id: "RC_2" },
							},
						})
						.pipe(Effect.exit);

					return {
						artifact: (yield* database.client
							.select()
							.from(HostedGitMutationArtifacts))[0],
						before_valid_result,
						competing,
						corrupt_journal,
						wrong_operation,
						wrong_origin,
					};
				}),
			);

			expect(failure_from(result.wrong_operation)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(failure_from(result.wrong_origin)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(failure_from(result.competing)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(failure_from(result.corrupt_journal)).toEqual(
				new HostedGitMutationInvariant({
					message: "Stored hosted Git mutation state is invalid",
				}),
			);
			expect(result.before_valid_result?.provider_result_json).toBeNull();
			expect(result.artifact?.provider_result_json).toBe(JSON.stringify(provider_result));
		} finally {
			await runtime.dispose();
		}
	});

	it("settles every terminal outcome once, publishes its public projection, and scrubs private state", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;
					const terminal = (
						identifier: string,
						settlement:
							| { readonly type: "applied" }
							| { readonly reason: "remote_rejected"; readonly type: "rejected" }
							| {
									readonly reason: "execution_interrupted";
									readonly type: "outcome_unknown";
							  },
					) =>
						Effect.gen(function* () {
							const approval_id = `approval_${identifier}`;

							yield* mutations.Request(identified_request(identifier));
							yield* mutations.Decide({
								approval_id,
								approved: true,
								decision_command: {
									message_id: `decision_${identifier}`,
									sent_at: later,
								},
								thread_id: "thread_1",
							});
							yield* mutations.MarkExecuting(approval_id);
							const execution = yield* mutations.ReadExecution(approval_id);
							const identity = { approval_id, claim_token: execution.claim_token };

							yield* mutations.ExecuteClaimed(identity, Effect.void);
							if (settlement.type === "applied") {
								yield* mutations.RecordProviderResult({
									...identity,
									result: provider_result,
								});
							}
							const settled = yield* mutations.Settle({ ...identity, ...settlement });
							const duplicate = yield* mutations.Settle({
								...identity,
								...settlement,
								claim_token: "stale_claim",
							});

							return { duplicate, settled };
						});

					yield* Seed;
					const applied = yield* terminal("applied", { type: "applied" });
					const rejected = yield* terminal("rejected", {
						reason: "remote_rejected",
						type: "rejected",
					});
					const unknown = yield* terminal("unknown", {
						reason: "execution_interrupted",
						type: "outcome_unknown",
					});

					return {
						applied,
						artifacts: yield* database.client.select().from(HostedGitMutationArtifacts),
						claims: yield* database.client.select().from(HostedGitMutationClaims),
						rejected,
						unknown,
					};
				}),
			);

			expect(result.applied.settled.approval).toMatchObject({
				result: provider_result,
				state: "applied",
			});
			expect(result.rejected.settled.approval).toMatchObject({
				reason: "remote_rejected",
				state: "rejected",
			});
			expect(result.unknown.settled.approval).toMatchObject({
				reason: "execution_interrupted",
				state: "outcome_unknown",
			});
			for (const outcome of [result.applied, result.rejected, result.unknown]) {
				expect(outcome.duplicate).toEqual({ ...outcome.settled, status: "duplicate" });
				expect(outcome.settled.event.payload).toMatchObject({
					approval: outcome.settled.approval,
					type: "hosted.git.mutation.approval.updated",
				});
			}
			expect(result.artifacts).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						operation_json: null,
						provider_result_json: null,
						selection_json: null,
					}),
				]),
			);
			expect(result.claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("requires durable launch and result evidence for terminal settlement", async () => {
		const runtime = make_runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;
					const launched = yield* Deferred.make<void>();

					yield* Seed;
					yield* mutations.Request(identified_request("prelaunch"));
					yield* mutations.Decide({
						approval_id: "approval_prelaunch",
						approved: true,
						decision_command: { message_id: "decision_prelaunch", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_prelaunch");
					const prelaunch = yield* mutations.ReadExecution("approval_prelaunch");
					const applied_without_result = yield* mutations
						.Settle({
							approval_id: "approval_prelaunch",
							claim_token: prelaunch.claim_token,
							type: "applied",
						})
						.pipe(Effect.exit);
					const unknown_without_launch = yield* mutations
						.Settle({
							approval_id: "approval_prelaunch",
							claim_token: prelaunch.claim_token,
							reason: "provider_outcome_unknown",
							type: "outcome_unknown",
						})
						.pipe(Effect.exit);
					const rejected_prelaunch = yield* mutations.Settle({
						approval_id: "approval_prelaunch",
						claim_token: prelaunch.claim_token,
						reason: "provider_unavailable",
						type: "rejected",
					});

					yield* mutations.Request(identified_request("interrupted"));
					yield* mutations.Decide({
						approval_id: "approval_interrupted",
						approved: true,
						decision_command: { message_id: "decision_interrupted", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_interrupted");
					const interrupted = yield* mutations.ReadExecution("approval_interrupted");
					const provider = yield* mutations
						.ExecuteClaimed(
							{
								approval_id: "approval_interrupted",
								claim_token: interrupted.claim_token,
							},
							Deferred.succeed(launched, undefined).pipe(
								Effect.andThen(Effect.never),
							),
						)
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Deferred.await(launched);
					yield* Fiber.interrupt(provider);
					const rejected_after_interruption = yield* mutations
						.Settle({
							approval_id: "approval_interrupted",
							claim_token: interrupted.claim_token,
							reason: "provider_unavailable",
							type: "rejected",
						})
						.pipe(Effect.exit);
					const unknown_after_interruption = yield* mutations.Settle({
						approval_id: "approval_interrupted",
						claim_token: interrupted.claim_token,
						reason: "execution_interrupted",
						type: "outcome_unknown",
					});

					return {
						applied_without_result,
						rejected_after_interruption,
						rejected_prelaunch,
						unknown_after_interruption,
						unknown_without_launch,
					};
				}),
			);

			expect(failure_from(result.applied_without_result)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(failure_from(result.unknown_without_launch)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(failure_from(result.rejected_after_interruption)).toEqual(
				new HostedGitMutationConflict({ reason: "artifact_conflict" }),
			);
			expect(result.rejected_prelaunch.approval).toMatchObject({
				reason: "provider_unavailable",
				state: "rejected",
			});
			expect(result.unknown_after_interruption.approval).toMatchObject({
				reason: "execution_interrupted",
				state: "outcome_unknown",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("classifies owned, waiting, recoverable, and quarantined executions from durable lease state", async () => {
		const [owned, waiting, prelaunch, quarantine, result_recorded] = await Promise.all([
			list_executing_for("owned"),
			list_executing_for("waiting"),
			list_executing_for("prelaunch"),
			list_executing_for("quarantine"),
			list_executing_for("result_recorded"),
		]);

		expect(owned).toEqual([
			{ approval_id: "approval_1", recovery: "owned", thread_id: "thread_1" },
		]);
		expect(waiting).toEqual([
			{ approval_id: "approval_1", recovery: "waiting", thread_id: "thread_1" },
		]);
		expect(prelaunch).toEqual([
			{ approval_id: "approval_1", recovery: "recoverable", thread_id: "thread_1" },
		]);
		expect(quarantine).toEqual([
			{ approval_id: "approval_1", recovery: "quarantine", thread_id: "thread_1" },
		]);
		expect(result_recorded).toEqual([
			{ approval_id: "approval_1", recovery: "recoverable", thread_id: "thread_1" },
		]);
	});

	it.each(["prelaunch", "result_recorded"] as const)(
		"claims %s recovery with one cross-runtime compare-and-swap winner",
		async (state) => {
			const database_path = await Effect.runPromise(MakeDatabasePath);
			const owner = make_runtime(database_path, `cas_owner_${state}`);
			const left = make_runtime(database_path, `cas_left_${state}`, {
				queued_values: [],
				value: later,
			});
			const right = make_runtime(database_path, `cas_right_${state}`, {
				queued_values: [],
				value: later,
			});

			try {
				await owner.runPromise(
					Effect.gen(function* () {
						const mutations = yield* HostedGitMutationRepository;

						yield* Seed;
						yield* mutations.Request(request());
						yield* mutations.Decide({
							approval_id: "approval_1",
							approved: true,
							decision_command: { message_id: "decision_approval_1", sent_at: later },
							thread_id: "thread_1",
						});
						yield* mutations.MarkExecuting("approval_1");
						const execution = yield* mutations.ReadExecution("approval_1");

						if (state === "result_recorded") {
							yield* mutations.ExecuteClaimed(
								{ approval_id: "approval_1", claim_token: execution.claim_token },
								Effect.void,
							);
							yield* mutations.RecordProviderResult({
								approval_id: "approval_1",
								claim_token: execution.claim_token,
								result: provider_result,
							});
						}
					}),
				);

				const recovered = await Promise.all([
					left.runPromise(
						Effect.flatMap(HostedGitMutationRepository, (mutations) =>
							mutations.ClaimRecovery("approval_1"),
						),
					),
					right.runPromise(
						Effect.flatMap(HostedGitMutationRepository, (mutations) =>
							mutations.ClaimRecovery("approval_1"),
						),
					),
				]);

				expect(recovered.filter(Option.isSome)).toHaveLength(1);
				const execution = Option.getOrThrow(recovered.find(Option.isSome)!);

				if (state === "result_recorded") {
					expect(execution.provider_result).toEqual(provider_result);
				} else {
					expect("provider_result" in execution).toBe(false);
				}
			} finally {
				await Promise.all([owner.dispose(), left.dispose(), right.dispose()]);
			}
		},
	);

	it("never reclaims a launched execution without a recorded result", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const owner = make_runtime(database_path, "launched_owner");
		const recovery = make_runtime(database_path, "launched_recovery", {
			queued_values: [],
			value: later,
		});

		try {
			await owner.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");

					yield* mutations.ExecuteClaimed(
						{ approval_id: "approval_1", claim_token: execution.claim_token },
						Effect.void,
					);
				}),
			);

			const claimed = await recovery.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.ClaimRecovery("approval_1"),
				),
			);

			expect(Option.isNone(claimed)).toBe(true);
		} finally {
			await Promise.all([owner.dispose(), recovery.dispose()]);
		}
	});

	it("quarantines only an expired launched execution without a provider result", async () => {
		const clock: TestClock = { queued_values: [], value: now };
		const runtime = make_runtime(
			await Effect.runPromise(MakeDatabasePath),
			"quarantine",
			clock,
		);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					const prelaunch = yield* mutations
						.QuarantineInterrupted("approval_1")
						.pipe(Effect.exit);

					yield* mutations.ExecuteClaimed(
						{ approval_id: "approval_1", claim_token: execution.claim_token },
						Effect.void,
					);
					const waiting = yield* mutations
						.QuarantineInterrupted("approval_1")
						.pipe(Effect.exit);
					clock.value = later;
					const quarantined = yield* mutations.QuarantineInterrupted("approval_1");

					return {
						artifacts: yield* database.client.select().from(HostedGitMutationArtifacts),
						claims: yield* database.client.select().from(HostedGitMutationClaims),
						prelaunch,
						quarantined,
						waiting,
					};
				}),
			);

			expect(failure_from(result.waiting)).toEqual(
				new HostedGitMutationConflict({ reason: "invalid_transition" }),
			);
			expect(Exit.isFailure(result.prelaunch)).toBe(true);
			expect(result.quarantined.approval).toMatchObject({
				reason: "execution_interrupted",
				state: "outcome_unknown",
			});
			expect(result.artifacts[0]).toMatchObject({
				operation_json: null,
				provider_result_json: null,
				selection_json: null,
			});
			expect(result.claims).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("holds terminal settlement and recovery behind the live provider workspace gate", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const provider_started = await Effect.runPromise(Deferred.make<void>());
		const provider_release = await Effect.runPromise(Deferred.make<void>());
		const owner = make_runtime(database_path, "gate_owner");
		const settlement = make_runtime(database_path, "gate_owner");
		const recovery = make_runtime(database_path, "gate_recovery", {
			queued_values: [],
			value: later,
		});
		let provider_execution: Promise<void> | undefined;

		try {
			const identity = await owner.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");

					return {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};
				}),
			);

			provider_execution = owner.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.ExecuteClaimed(
						identity,
						Deferred.succeed(provider_started, undefined).pipe(
							Effect.andThen(Deferred.await(provider_release)),
						),
					),
				),
			);
			await Effect.runPromise(Deferred.await(provider_started));

			const blocked_settlement = await settlement.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.Settle({
						...identity,
						reason: "execution_interrupted",
						type: "outcome_unknown",
					}),
				).pipe(Effect.timeoutOption("100 millis")),
			);
			const blocked_quarantine = await recovery.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.QuarantineInterrupted("approval_1"),
				).pipe(Effect.timeoutOption("100 millis")),
			);
			const during_provider = await recovery.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.Query({ approval_id: "approval_1", thread_id: "thread_1" }),
				),
			);

			expect(Option.isNone(blocked_settlement)).toBe(true);
			expect(Option.isNone(blocked_quarantine)).toBe(true);
			expect(during_provider.approval.state).toBe("executing");

			await Effect.runPromise(Deferred.succeed(provider_release, undefined));
			await provider_execution;
			provider_execution = undefined;

			const quarantined = await recovery.runPromise(
				Effect.flatMap(HostedGitMutationRepository, (mutations) =>
					mutations.QuarantineInterrupted("approval_1"),
				),
			);

			expect(quarantined.approval).toMatchObject({
				reason: "execution_interrupted",
				state: "outcome_unknown",
			});
		} finally {
			await Effect.runPromise(Deferred.succeed(provider_release, undefined));
			await provider_execution?.catch(() => undefined);
			await Promise.all([owner.dispose(), recovery.dispose(), settlement.dispose()]);
		}
	});

	it("abandons only leases owned by the current runtime", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock: TestClock = { queued_values: [], value: now };
		const owner = make_runtime(database_path, "abandon_owner", clock);
		const foreign = make_runtime(database_path, "abandon_foreign");

		try {
			const result = await owner.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					yield* mutations.AbandonOwnedExecutions;
					const [abandoned] = yield* database.client
						.select()
						.from(HostedGitMutationClaims);
					yield* database.client
						.update(HostedGitMutationClaims)
						.set({ owner_instance_id: "abandon_foreign", lease_expires_at: later });
					yield* mutations.AbandonOwnedExecutions;

					return {
						abandoned,
						foreign: (yield* database.client.select().from(HostedGitMutationClaims))[0],
					};
				}),
			);

			expect(result.abandoned).toMatchObject({
				owner_instance_id: "abandon_owner",
				lease_expires_at: "2026-07-16T12:00:00.001Z",
			});
			expect(result.foreign).toMatchObject({
				owner_instance_id: "abandon_foreign",
				lease_expires_at: later,
			});
		} finally {
			await Promise.all([owner.dispose(), foreign.dispose()]);
		}
	});

	it("does not invoke the provider after restart when a result is already recorded", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const clock: TestClock = { queued_values: [], value: now };
		const initial = make_runtime(database_path, "recording_owner", clock);
		let provider_invocations = 0;

		try {
			await initial.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					yield* mutations.Request(request());
					yield* mutations.Decide({
						approval_id: "approval_1",
						approved: true,
						decision_command: { message_id: "decision_approval_1", sent_at: later },
						thread_id: "thread_1",
					});
					yield* mutations.MarkExecuting("approval_1");
					const execution = yield* mutations.ReadExecution("approval_1");
					const identity = {
						approval_id: "approval_1",
						claim_token: execution.claim_token,
					};

					yield* mutations.ExecuteClaimed(identity, Effect.void);
					yield* mutations.RecordProviderResult({ ...identity, result: provider_result });
				}),
			);
		} finally {
			await initial.dispose();
		}

		clock.value = later;
		const restarted = make_runtime(database_path, "recording_recovery", clock);
		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const mutations = yield* HostedGitMutationRepository;
					const execution = Option.getOrThrow(
						yield* mutations.ClaimRecovery("approval_1"),
					);
					const replay = yield* mutations
						.ExecuteClaimed(
							{ approval_id: "approval_1", claim_token: execution.claim_token },
							Effect.sync(() => ++provider_invocations),
						)
						.pipe(Effect.exit);
					const settled = yield* mutations.Settle({
						approval_id: "approval_1",
						claim_token: execution.claim_token,
						type: "applied",
					});

					return { execution, replay, settled };
				}),
			);

			expect(result.execution.provider_result).toEqual(provider_result);
			expect(failure_from(result.replay)).toEqual(
				new HostedGitMutationConflict({ reason: "lease_conflict" }),
			);
			expect(result.settled.approval).toMatchObject({
				result: provider_result,
				state: "applied",
			});
			expect(provider_invocations).toBe(0);
		} finally {
			await restarted.dispose();
		}
	});
});
