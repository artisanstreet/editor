import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Cause, Effect, Exit, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	ExternalWaitGate,
	HostedGitCheck,
	HostedGitSnapshotQueryResult,
} from "@artisan/protocol";

import {
	ExternalWaitDispatcher,
	ExternalWaitDispatcherFailure,
	type ExternalWaitDispatchCycleResult,
} from "../../modules/backend/src/external-wait/external-wait-dispatcher";
import { ExternalWaitRepositoryLive } from "../../modules/backend/src/external-wait/external-wait-repository";
import {
	ExternalWaitService,
	ExternalWaitServiceLive,
	type ExternalWaitRequestCommand,
} from "../../modules/backend/src/external-wait/external-wait-service";
import {
	HostedGitSnapshotService,
	HostedGitSnapshotServiceFailure,
} from "../../modules/backend/src/git-provider/hosted-git-snapshot-service";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	ExternalWaits,
	OrchestrationGroups,
	OrchestrationRuns,
	ProjectHostedOrigins,
	Projects,
	Threads,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const now = "2026-07-14T15:00:00.000Z";
const head = "b".repeat(40);

interface HostedState {
	result: HostedGitSnapshotQueryResult;
	failure?: HostedGitSnapshotServiceFailure;
}

interface DispatcherState {
	calls: number;
	fails: boolean;
}

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-external-wait-service-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer() {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "external_wait_service_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_external_wait_service_${++identifier}`),
		Now: Effect.succeed(now),
	});
}

function check(state: HostedGitCheck["state"]): HostedGitCheck {
	return {
		annotations: [],
		annotations_truncated: false,
		name: "build",
		origin: { native_id: "check_1", provider_id: "github", resource_kind: "check_run" },
		required: true,
		state,
	};
}

function hosted_result(
	options: {
		readonly association?: "matched" | "none";
		readonly check_state?: HostedGitCheck["state"];
		readonly freshness?: "current" | "stale_local_git";
	} = {},
): HostedGitSnapshotQueryResult {
	const association = options.association ?? "matched";
	const lookup = {
		association:
			association === "none"
				? ({ _tag: "none" } as const)
				: {
						_tag: "matched" as const,
						freshness: "current" as const,
						pull_request: {
							base_branch: "main",
							base_commit: "a".repeat(40),
							checks: [check(options.check_state ?? "running")],
							checks_total: 1,
							checks_truncated: false,
							draft: false,
							head_branch: "main",
							head_commit: head,
							mergeability: "mergeable" as const,
							number: 7,
							origin: {
								native_id: "pr_7",
								provider_id: "github",
								resource_kind: "pull_request" as const,
							},
							requested_reviewers: [],
							requested_reviewers_truncated: false,
							review_decision: "none" as const,
							review_threads: [],
							review_threads_total: 0,
							review_threads_truncated: false,
							reviews: [],
							reviews_total: 0,
							reviews_truncated: false,
							state: "open" as const,
							title: "External wait",
							web_url: "https://github.com/artisan/editor/pull/7",
						},
					},
		branch: "main",
		expected_head_commit: head,
		repository: { host: "github.com", name: "editor", owner: "artisan", provider_id: "github" },
	};

	return {
		journal_sequence: 1,
		snapshot: {
			journal_sequence: 1,
			lookup,
			observed_at: now,
			project_id: "project_1",
			version: 1,
			workspace_freshness: options.freshness ?? "current",
			workspace_id: "workspace_1",
		},
	};
}

function runtime(
	database_path: string,
	hosted_state: HostedState,
	dispatcher_state: DispatcherState,
) {
	const hosted_git = Layer.succeed(HostedGitSnapshotService, {
		Query: () =>
			hosted_state.failure === undefined
				? Effect.succeed(hosted_state.result)
				: Effect.fail(hosted_state.failure),
		ReadCurrent: () => {
			if (hosted_state.failure !== undefined) {
				return Effect.fail(hosted_state.failure);
			}

			const snapshot = hosted_state.result.snapshot;

			return snapshot === undefined || snapshot.workspace_freshness !== "current"
				? Effect.fail(
						new HostedGitSnapshotServiceFailure({ reason: "workspace_unavailable" }),
					)
				: Effect.succeed({
						lookup: snapshot.lookup,
						observed_at: snapshot.observed_at,
						project_id: snapshot.project_id,
						workspace_id: snapshot.workspace_id,
					});
		},
		Refresh: () => Effect.die("unused"),
	});
	const dispatcher = Layer.succeed(ExternalWaitDispatcher, {
		RunOnce: Effect.suspend(() => {
			dispatcher_state.calls += 1;

			if (dispatcher_state.fails) {
				return Effect.fail(
					new ExternalWaitDispatcherFailure({ cause: "dispatcher unavailable" }),
				);
			}

			return Effect.succeed({
				materialized_outbox_ids: [],
				released_or_skipped_outbox_ids: [],
			} satisfies ExternalWaitDispatchCycleResult);
		}),
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(),
		JournalNotifierLive,
		NodeCrypto.layer,
	);
	const repository = ExternalWaitRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const service = ExternalWaitServiceLive.pipe(
		Layer.provideMerge(infrastructure),
		Layer.provideMerge(repository),
		Layer.provideMerge(hosted_git),
		Layer.provideMerge(dispatcher),
	);

	return ManagedRuntime.make(service);
}

const Seed = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(Projects).values({
		canonical_root: "C:/artisan",
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
		selected_account_login: "sander",
		web_url: "https://github.com/artisan/editor",
	});
	yield* database.client.insert(Threads).values({
		created_at: now,
		primary_project_id: "project_1",
		primary_project_json: JSON.stringify({
			display_name: "Artisan",
			project_id: "project_1",
			root_path: "C:/artisan",
		}),
		thread_id: "thread_1",
		title: "External wait",
		title_source: "initial",
		updated_at: now,
	});
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "agent_1",
		created_at: now,
		engine_id: "codex",
		run_id: "run_1",
		status: "running",
		thread_id: "thread_1",
		updated_at: now,
		working_directory: "C:/artisan",
	});
});

const SeedGraphRun = (thread_id = "thread_1") =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(OrchestrationGroups).values({
			coordinator_agent_id: "coordinator_1",
			created_at: now,
			group_id: "group_1",
			journal_sequence: 1,
			max_concurrency: 1,
			state: "running",
			thread_id,
			updated_at: now,
			version: 1,
		});
		yield* database.client.insert(AgentInstances).values({
			agent_id: "graph_agent_1",
			created_at: now,
			display_name: "Graph worker",
			group_id: "group_1",
			role: "worker",
			updated_at: now,
		});
		yield* database.client.insert(Assignments).values({
			active_run_id: "graph_run_1",
			agent_id: "graph_agent_1",
			assignment_id: "assignment_1",
			created_at: now,
			current_attempt: 1,
			engine_id: "codex",
			expected_result: "result",
			group_id: "group_1",
			instructions: "Wait for external checks.",
			max_attempts: 1,
			parent_node_id: "node_1",
			permission_policy_json: JSON.stringify({
				approval: "on_request",
				network_access: false,
				write_access: true,
			}),
			profile: "default",
			role: "worker",
			scope_json: JSON.stringify({ kind: "files", value: "src", write_access: true }),
			state: "running",
			summary_contract: "summary",
			updated_at: now,
			workspace_json: JSON.stringify({
				isolation: "shared",
				working_directory: "C:/artisan",
				workspace_id: "workspace_1",
			}),
		});
		yield* database.client.insert(AgentRuns).values({
			agent_id: "graph_agent_1",
			assignment_id: "assignment_1",
			attempt: 1,
			created_at: now,
			dispatch_status: "active",
			engine_id: "codex",
			group_id: "group_1",
			last_observation_sequence: 0,
			profile: "default",
			run_id: "graph_run_1",
			state: "running",
			updated_at: now,
		});
	});

function request(
	overrides: Partial<{
		gates: ReadonlyArray<ExternalWaitGate>;
		message_id: string;
		source_run_id: string;
		thread_id: string;
	}> = {},
): ExternalWaitRequestCommand {
	return {
		expected_head_commit: head,
		gates: overrides.gates ?? [{ _tag: "required_checks_terminal" }],
		message_id: overrides.message_id ?? "request_1",
		pull_request_number: 7,
		sent_at: now,
		source_run_id: overrides.source_run_id ?? "run_1",
		thread_id: overrides.thread_id ?? "thread_1",
		workspace_id: "workspace_1",
	};
}

function failure_from(exit: Exit.Exit<unknown, unknown>) {
	if (Exit.isFailure(exit)) {
		return Cause.squash(exit.cause);
	}

	throw new Error("Expected the Effect to fail");
}

afterEach(async () => {
	await Effect.runPromise(
		Effect.forEach(
			directories.splice(0),
			(directory) =>
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("ExternalWaitService", () => {
	it("derives durable ownership, target, and baseline from a current hosted snapshot", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ExternalWaitService;

					yield* Seed;
					const accepted = yield* service.Request(request());
					const query = yield* service.Query({ thread_id: "thread_1" });
					const runs = yield* database.client.select().from(OrchestrationRuns);

					return { accepted, query, runs };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.accepted.snapshot.owner).toMatchObject({
				_tag: "thread_run",
				run_id: "run_1",
			});
			expect(result.accepted.snapshot.target).toMatchObject({
				expected_head_commit: head,
				pull_request_number: 7,
			});
			expect(result.query.snapshots[0]?.state).toEqual({ _tag: "waiting" });
			expect(result.runs[0]?.status).toBe("waiting_external");
		} finally {
			await instance.dispose();
		}
	});

	it("registers a wait for an active graph assignment and transitions its ownership chain", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ExternalWaitService;

					yield* Seed;
					yield* SeedGraphRun();
					const accepted = yield* service.Request(
						request({ message_id: "graph_request_1", source_run_id: "graph_run_1" }),
					);
					const [[assignment], [run]] = yield* Effect.all([
						database.client.select().from(Assignments),
						database.client.select().from(AgentRuns),
					]);

					return { accepted, assignment, run };
				}),
			);

			expect(result.accepted.snapshot.owner).toEqual({
				_tag: "assignment_run",
				agent_id: "graph_agent_1",
				assignment_id: "assignment_1",
				engine_id: "codex",
				group_id: "group_1",
				run_id: "graph_run_1",
			});
			expect(result.assignment?.state).toBe("waiting_external");
			expect(result.run).toMatchObject({
				dispatch_status: "waiting_external",
				state: "waiting_external",
			});
		} finally {
			await instance.dispose();
		}
	});

	it("rejects a graph run owned by another thread without transitioning its assignment", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ExternalWaitService;

					yield* Seed;
					yield* SeedGraphRun("thread_2");
					const failed = yield* Effect.exit(
						service.Request(
							request({
								message_id: "cross_thread_graph_request",
								source_run_id: "graph_run_1",
							}),
						),
					);
					const [[assignment], [run], waits] = yield* Effect.all([
						database.client.select().from(Assignments),
						database.client.select().from(AgentRuns),
						database.client.select().from(ExternalWaits),
					]);

					return { assignment, failed, run, waits };
				}),
			);

			expect(failure_from(result.failed)).toMatchObject({
				_tag: "ExternalWaitServiceFailure",
				reason: "source_run_unavailable",
			});
			expect(result.assignment?.state).toBe("running");
			expect(result.run).toMatchObject({ dispatch_status: "active", state: "running" });
			expect(result.waits).toEqual([]);
		} finally {
			await instance.dispose();
		}
	});

	it("preserves an initial branch change failure without creating a wait", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{
				failure: new HostedGitSnapshotServiceFailure({ reason: "branch_changed" }),
				result: hosted_result(),
			},
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ExternalWaitService;

					yield* Seed;
					const failed = yield* Effect.exit(service.Request(request()));
					const [[run], waits] = yield* Effect.all([
						database.client.select().from(OrchestrationRuns),
						database.client.select().from(ExternalWaits),
					]);

					return { failed, run, waits };
				}),
			);

			expect(failure_from(result.failed)).toEqual(
				new HostedGitSnapshotServiceFailure({ reason: "branch_changed" }),
			);
			expect(result.waits).toEqual([]);
			expect(result.run?.status).toBe("running");
		} finally {
			await instance.dispose();
		}
	});

	it("replays an exact request after hosted state changes, while rejecting changed intent", async () => {
		const hosted_state: HostedState = { result: hosted_result() };
		const instance = runtime(await Effect.runPromise(MakeDatabasePath), hosted_state, {
			calls: 0,
			fails: false,
		});

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const service = yield* ExternalWaitService;

					yield* Seed;
					const accepted = yield* service.Request(request());
					hosted_state.failure = new HostedGitSnapshotServiceFailure({
						reason: "provider_unavailable",
					});
					const replay = yield* service.Request(request());
					const changed = yield* Effect.exit(
						service.Request(request({ gates: [{ _tag: "review_decision_changed" }] })),
					);

					return { accepted, changed, replay };
				}),
			);

			expect(result.replay).toEqual({ ...result.accepted, status: "duplicate" });
			expect(failure_from(result.changed)).toMatchObject({ _tag: "ExternalWaitConflict" });
		} finally {
			await instance.dispose();
		}
	});

	it("refuses satisfied, stale, and unmatched snapshots without closing the source run", async () => {
		for (const [name, result] of [
			["satisfied", hosted_result({ check_state: "passed" })],
			["stale", hosted_result({ freshness: "stale_local_git" })],
			["unmatched", hosted_result({ association: "none" })],
		] as const) {
			const instance = runtime(
				await Effect.runPromise(MakeDatabasePath),
				{ result },
				{ calls: 0, fails: false },
			);

			try {
				const outcome = await instance.runPromise(
					Effect.gen(function* () {
						const database = yield* Database;
						const service = yield* ExternalWaitService;

						yield* Seed;
						const failed = yield* Effect.exit(service.Request(request()));
						const [run] = yield* database.client.select().from(OrchestrationRuns);

						return { failed, run };
					}),
				);

				expect(Exit.isFailure(outcome.failed), name).toBe(true);
				expect(outcome.run?.status, name).toBe("running");
			} finally {
				await instance.dispose();
			}
		}
	});

	it("cancels visibly and replays the original cancellation outcome", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const service = yield* ExternalWaitService;

					yield* Seed;
					yield* service.Request(request());
					const input = {
						message_id: "cancel_1",
						sent_at: now,
						thread_id: "thread_1",
						wait_id: "request_1",
					};
					const accepted = yield* service.Cancel(input);
					const duplicate = yield* service.Cancel(input);
					const query = yield* service.Query({ thread_id: "thread_1" });

					return { accepted, duplicate, query };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.query.snapshots[0]?.state).toEqual({ _tag: "cancelled", reason: "user" });
		} finally {
			await instance.dispose();
		}
	});

	it("resumes manually despite dispatcher failure and replays the durable wake", async () => {
		const dispatcher_state = { calls: 0, fails: true };
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			dispatcher_state,
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const service = yield* ExternalWaitService;

					yield* Seed;
					yield* service.Request(request());
					const input = {
						message_id: "resume_1",
						sent_at: now,
						thread_id: "thread_1",
						wait_id: "request_1",
					};
					const accepted = yield* service.ManualResume(input);
					const duplicate = yield* service.ManualResume(input);

					return { accepted, duplicate };
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.duplicate.status).toBe("duplicate");
			expect(result.accepted.wake).toEqual(result.duplicate.wake);
			expect(dispatcher_state.calls).toBe(2);
		} finally {
			await instance.dispose();
		}
	});

	it("fails when the source run belongs to another thread", async () => {
		const instance = runtime(
			await Effect.runPromise(MakeDatabasePath),
			{ result: hosted_result() },
			{ calls: 0, fails: false },
		);

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const service = yield* ExternalWaitService;

					yield* Seed;
					const failed = yield* Effect.exit(
						service.Request(request({ thread_id: "thread_2" })),
					);
					const [run] = yield* database.client.select().from(OrchestrationRuns);

					return { failed, run };
				}),
			);

			expect(failure_from(result.failed)).toMatchObject({
				_tag: "ExternalWaitServiceFailure",
				reason: "source_run_unavailable",
			});
			expect(result.run?.status).toBe("running");
		} finally {
			await instance.dispose();
		}
	});
});
