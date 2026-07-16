import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Deferred, Effect, Fiber, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitProvider,
	GitProviderError,
	type GitProviderErrorReason,
} from "../../modules/backend/src/git-provider/git-provider";
import {
	HostedGitMutationCoordinator,
	HostedGitMutationCoordinatorLive,
} from "../../modules/backend/src/git-provider/hosted-git-mutation-coordinator";
import { make_git_provider_registry_layer } from "../../modules/backend/src/git-provider/git-provider-registry";
import {
	HostedGitMutationRepository,
	HostedGitMutationRepositoryLive,
	HostedGitMutationInvariant,
} from "../../modules/backend/src/git-provider/hosted-git-mutation-repository";
import { make_workspace_git_execution_gate_layer } from "../../modules/backend/src/git/workspace-git-execution-gate";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	HostedGitSnapshots,
	ProjectHostedOrigins,
	Projects,
	Threads,
	WorkspaceGitSessions,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];
const now = "2026-07-16T12:00:00.000Z";
const later = "2026-07-16T12:01:00.000Z";
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

interface ProviderState {
	calls: number;
	entered?: Deferred.Deferred<void>;
	resume?: Deferred.Deferred<void>;
	reason?: GitProviderErrorReason;
}

interface RuntimeOptions {
	current_time?: string;
	fail_provider_result?: boolean;
	fail_settle?: boolean;
	instance_id?: string;
	settlement_attempted?: Deferred.Deferred<void>;
}

let runtime_sequence = 0;

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-hosted-git-mutation-coordinator-",
	});

	yield* Effect.sync(() => temporary_directories.push(directory));

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function snapshot_lookup() {
	return {
		association: {
			_tag: "matched" as const,
			freshness: "current" as const,
			pull_request: {
				base_branch: "main",
				base_commit: "b".repeat(40),
				checks: [],
				checks_total: 0,
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
	yield* database.client.insert(Threads).values({
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
	});
	yield* database.client.insert(HostedGitSnapshots).values({
		journal_sequence: 1,
		lookup_json: JSON.stringify(lookup),
		observed_at: now,
		project_id: "project_1",
		version: 1,
	});
});

function request(message_id = "request_1", body = "Private reply") {
	return {
		message_id,
		mutation: {
			body,
			expected_head_commit: "a".repeat(40),
			operation: "reply_review_thread" as const,
			pull_request_number: 42,
			pull_request_origin,
			repository,
			selected_branch: "feature",
			snapshot_version: 1,
			thread_origin: review_thread_origin,
			workspace_id: "workspace_1",
		},
		selection,
		sent_at: now,
		thread_id: "thread_1",
	};
}

function decision(approval_id: string, message_id = "decision_1") {
	return {
		approval_id,
		approved: true,
		message_id,
		sent_at: later,
		thread_id: "thread_1",
	};
}

function make_provider(state: ProviderState, include_mutation = true) {
	const ExecuteMutation = (
		input: Parameters<NonNullable<typeof GitProvider.Service.ExecuteMutation>>[0],
	) =>
		Effect.gen(function* () {
			state.calls += 1;

			if (state.entered) {
				yield* Deferred.succeed(state.entered, undefined);
			}
			if (state.resume) {
				yield* Deferred.await(state.resume);
			}
			if (state.reason) {
				return yield* new GitProviderError({
					host: "github.com",
					operation: "execute_mutation",
					provider_id: "github",
					reason: state.reason,
					retryable: false,
				});
			}

			return {
				operation: "reply_review_thread" as const,
				origin: {
					native_id: `RC_${input.client_mutation_id}`,
					provider_id: "github",
					resource_kind: "review_comment" as const,
				},
				status: "applied" as const,
				thread_origin: review_thread_origin,
			};
		});

	return {
		Clone: () => Effect.die("Clone is outside hosted mutation coordinator tests"),
		Descriptor: {
			capabilities: [
				{ _tag: "available" as const, capability: "write_provider_mutations" as const },
			],
			display_name: "GitHub",
			provider_id: "github",
		},
		DiscoverRepositories: () =>
			Effect.die("Discovery is outside hosted mutation coordinator tests"),
		Inspect: Effect.die("Inspection is outside hosted mutation coordinator tests"),
		PrepareClone: () => Effect.die("Preparation is outside hosted mutation coordinator tests"),
		...(include_mutation ? { ExecuteMutation } : {}),
	} satisfies typeof GitProvider.Service;
}

function make_runtime(
	database_path: string,
	provider?: typeof GitProvider.Service,
	options: RuntimeOptions = {},
) {
	let next_id = 0;
	const instance_id =
		options.instance_id ?? `hosted_git_mutation_coordinator_${++runtime_sequence}`;
	const infrastructure = Layer.mergeAll(
		NodeCrypto.layer,
		make_database_layer({ database_path, migrations_path }),
		make_workspace_git_execution_gate_layer({ database_path }),
		Layer.succeed(RuntimeMetadata, {
			instance_id,
			MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
			Now: Effect.succeed(options.current_time ?? now),
		}),
		JournalNotifierLive,
	);
	const live_repository = HostedGitMutationRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const coordinator_repository =
		options.fail_settle || options.fail_provider_result
			? Layer.effect(
					HostedGitMutationRepository,
					Effect.gen(function* () {
						const repository = yield* HostedGitMutationRepository;
						let should_fail = true;

						return {
							...repository,
							...(options.fail_provider_result
								? {
										RecordProviderResult: () =>
											Effect.fail(
												new HostedGitMutationInvariant({
													message: "Forced provider result failure",
												}),
											),
									}
								: {}),
							Settle: (input) => {
								if (options.fail_settle && should_fail) {
									should_fail = false;

									return (
										options.settlement_attempted
											? Deferred.succeed(
													options.settlement_attempted,
													undefined,
												)
											: Effect.void
									).pipe(
										Effect.andThen(
											Effect.fail(
												new HostedGitMutationInvariant({
													message: "Forced settlement failure",
												}),
											),
										),
									);
								}

								return repository.Settle(input);
							},
						};
					}),
				).pipe(Layer.provide(live_repository))
			: live_repository;
	const services = Layer.mergeAll(
		infrastructure,
		coordinator_repository,
		make_git_provider_registry_layer(
			provider === undefined ? [] : [{ hosts: ["github.com"], provider }],
		),
	);

	const coordinator = HostedGitMutationCoordinatorLive.pipe(Layer.provide(services));

	return ManagedRuntime.make(Layer.merge(services, coordinator));
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

describe("HostedGitMutationCoordinator", () => {
	it("executes an approved exact request once and replays without provider work", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state = { calls: 0 };
		const runtime = make_runtime(database_path, make_provider(state));

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					const accepted = yield* coordinator.Request(request());

					yield* coordinator.Respond(decision(accepted.approval.approval_id));
					yield* coordinator.AwaitIdle;

					const replay = yield* coordinator.Request(request());
					const query = yield* mutations.Query({
						approval_id: accepted.approval.approval_id,
						thread_id: "thread_1",
					});

					return { query, replay };
				}),
			);

			expect(state.calls).toBe(1);
			expect(result.replay.status).toBe("duplicate");
			expect(result.query.approval).toMatchObject({
				state: "applied",
				result: { status: "applied" },
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers a recorded provider result after a settlement failure without replaying it", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state = { calls: 0 };
		const settlement_attempted = await Effect.runPromise(Deferred.make<void>());
		const failing = make_runtime(database_path, make_provider(state), {
			fail_settle: true,
			settlement_attempted,
		});

		try {
			await failing.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;

					yield* Seed;
					const accepted = yield* coordinator.Request(request());

					yield* coordinator.Respond(decision(accepted.approval.approval_id));
					yield* Deferred.await(settlement_attempted);
				}),
			);
		} finally {
			await failing.dispose();
		}

		const recovered = make_runtime(database_path, make_provider(state), {
			current_time: later,
		});

		try {
			const query = await recovered.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;
					const mutations = yield* HostedGitMutationRepository;

					yield* coordinator.Recover;
					yield* coordinator.AwaitIdle;

					return yield* mutations.Query({
						approval_id: "hosted_git_mutation:request_1",
						thread_id: "thread_1",
					});
				}),
			);

			expect(state.calls).toBe(1);
			expect(query.approval).toMatchObject({
				state: "applied",
				result: { status: "applied" },
			});
		} finally {
			await recovered.dispose();
		}
	});

	it("quarantines an applied provider call when its result cannot be recorded", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const state = { calls: 0 };
		const runtime = make_runtime(database_path, make_provider(state), {
			fail_provider_result: true,
		});

		try {
			const query = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					const accepted = yield* coordinator.Request(request());

					yield* coordinator.Respond(decision(accepted.approval.approval_id));
					yield* coordinator.AwaitIdle;

					return yield* mutations.Query({
						approval_id: accepted.approval.approval_id,
						thread_id: "thread_1",
					});
				}),
			);

			expect(state.calls).toBe(1);
			expect(query.approval).toMatchObject({
				reason: "provider_outcome_unknown",
				state: "outcome_unknown",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it.each([
		["permission_denied", "rejected", "permission_denied", true],
		["auth_required", "rejected", "authentication_required", true],
		["rate_limited", "rejected", "rate_limited", true],
		["remote_rejected", "rejected", "remote_rejected", true],
		["stale_repository", "rejected", "snapshot_stale", true],
		["invalid_response", "rejected", "invalid_provider_response", true],
		["network", "rejected", "provider_unavailable", true],
		["outcome_unknown", "outcome_unknown", "provider_outcome_unknown", true],
		[undefined, "rejected", "unsupported_operation", false],
	] as const)(
		"settles %s without provider replay",
		async (reason, state_name, outcome, execute) => {
			const database_path = await Effect.runPromise(MakeDatabasePath);
			const state = { calls: 0, ...(reason === undefined ? {} : { reason }) };
			const runtime = make_runtime(database_path, make_provider(state, execute));

			try {
				const query = await runtime.runPromise(
					Effect.gen(function* () {
						const coordinator = yield* HostedGitMutationCoordinator;
						const mutations = yield* HostedGitMutationRepository;

						yield* Seed;
						const accepted = yield* coordinator.Request(request());

						yield* coordinator.Respond(decision(accepted.approval.approval_id));
						yield* coordinator.AwaitIdle;

						return yield* mutations.Query({
							approval_id: accepted.approval.approval_id,
							thread_id: "thread_1",
						});
					}),
				);

				expect(query.approval).toMatchObject({ reason: outcome, state: state_name });
				expect(state.calls).toBe(execute ? 1 : 0);
			} finally {
				await runtime.dispose();
			}
		},
	);

	it("rejects an approved mutation when its selected provider is unavailable", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);

		try {
			const query = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					const accepted = yield* coordinator.Request(request());

					yield* coordinator.Respond(decision(accepted.approval.approval_id));
					yield* coordinator.AwaitIdle;

					return yield* mutations.Query({
						approval_id: accepted.approval.approval_id,
						thread_id: "thread_1",
					});
				}),
			);

			expect(query.approval).toMatchObject({
				reason: "provider_unavailable",
				state: "rejected",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("queues explicit recovery behind a live provider execution", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const [entered, resume] = await Effect.runPromise(
			Effect.all([Deferred.make<void>(), Deferred.make<void>()]),
		);
		const state = {
			calls: 0,
			entered,
			reason: "permission_denied" as const,
			resume,
		};
		const runtime = make_runtime(database_path, make_provider(state));

		try {
			const query = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;
					const mutations = yield* HostedGitMutationRepository;

					yield* Seed;
					const accepted = yield* coordinator.Request(request());

					yield* coordinator.Respond(decision(accepted.approval.approval_id));
					yield* Deferred.await(entered);
					yield* coordinator.Recover;
					yield* Deferred.succeed(resume, undefined);
					yield* coordinator.AwaitIdle;

					return yield* mutations.Query({
						approval_id: accepted.approval.approval_id,
						thread_id: "thread_1",
					});
				}),
			);

			expect(state.calls).toBe(1);
			expect(query.approval).toMatchObject({
				reason: "permission_denied",
				state: "rejected",
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("drains a live provider execution and fences later same-thread dispatch", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const [entered, resume] = await Effect.runPromise(
			Effect.all([Deferred.make<void>(), Deferred.make<void>()]),
		);
		const state = { calls: 0, entered, resume };
		const runtime = make_runtime(database_path, make_provider(state));

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;

					yield* Seed;
					const first = yield* coordinator.Request(request());
					yield* coordinator.Respond(decision(first.approval.approval_id));
					yield* Deferred.await(entered);

					const quiesce = yield* coordinator
						.QuiesceThread("thread_1")
						.pipe(Effect.forkChild({ startImmediately: true }));

					yield* Deferred.succeed(resume, undefined);
					yield* Fiber.join(quiesce);
					yield* coordinator.AwaitIdle;

					const second = yield* coordinator.Request(request("request_2"));
					yield* coordinator.Respond(decision(second.approval.approval_id, "decision_2"));
					yield* coordinator.AwaitIdle;
				}),
			);

			expect(state.calls).toBe(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects changed intent for a reused source message", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path, make_provider({ calls: 0 }));

		try {
			const error = await runtime.runPromiseExit(
				Effect.gen(function* () {
					const coordinator = yield* HostedGitMutationCoordinator;

					yield* Seed;
					yield* coordinator.Request(request());

					return yield* coordinator.Request(
						request("request_1", "Changed private reply"),
					);
				}),
			);

			expect(error._tag).toBe("Failure");
		} finally {
			await runtime.dispose();
		}
	});
});
