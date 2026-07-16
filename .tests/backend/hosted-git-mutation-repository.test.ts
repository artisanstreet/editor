import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Deferred, Effect, Exit, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	HostedGitMutationConflict,
	HostedGitMutationInvariant,
	HostedGitMutationRepository,
	HostedGitMutationRepositoryLive,
	HostedGitMutationUnavailable,
} from "../../modules/backend/src/git-provider/hosted-git-mutation-repository";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
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

	return ManagedRuntime.make(
		HostedGitMutationRepositoryLive.pipe(Layer.provideMerge(infrastructure)),
	);
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
});
