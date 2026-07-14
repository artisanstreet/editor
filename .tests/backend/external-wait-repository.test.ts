import { fileURLToPath } from "node:url";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ExternalWaitRepository,
	ExternalWaitRepositoryLive,
	type ExternalWaitRegistration,
} from "../../modules/backend/src/external-wait/external-wait-repository";
import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import {
	AgentInstances,
	AgentRuns,
	Assignments,
	ExternalWaitOperations,
	ExternalWaits,
	ExternalWaitWakeOutbox,
	JournalEvents,
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

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({ prefix: "artisan-external-wait-" });

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function metadata_layer() {
	let identifier = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "external_wait_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_external_wait_${++identifier}`),
		Now: Effect.succeed(now),
	});
}

function runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		metadata_layer(),
		JournalNotifierLive,
		NodeCrypto.layer,
	);

	return ManagedRuntime.make(ExternalWaitRepositoryLive.pipe(Layer.provideMerge(infrastructure)));
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

const SeedRun = (run_id: string) =>
	Effect.gen(function* () {
		const database = yield* Database;

		yield* database.client.insert(OrchestrationRuns).values({
			agent_id: "agent_1",
			created_at: now,
			engine_id: "codex",
			run_id,
			status: "running",
			thread_id: "thread_1",
			updated_at: now,
			working_directory: "C:/artisan",
		});
	});

const SeedGraphRun = Effect.gen(function* () {
	const database = yield* Database;

	yield* database.client.insert(OrchestrationGroups).values({
		coordinator_agent_id: "coordinator_1",
		created_at: now,
		group_id: "group_1",
		journal_sequence: 1,
		max_concurrency: 1,
		state: "running",
		thread_id: "thread_1",
		updated_at: now,
		version: 1,
	});
	yield* database.client.insert(AgentInstances).values({
		agent_id: "graph_agent_1",
		created_at: now,
		display_name: "Gibby",
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
		instructions: "instructions",
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

function registration_for(
	overrides: {
		readonly command_id?: string;
		readonly maximum_generation?: number;
		readonly request_fingerprint?: string;
		readonly run_id?: string;
		readonly sent_at?: string;
		readonly thread_id?: string;
		readonly wait_id?: string;
	} = {},
): ExternalWaitRegistration {
	const run_id = overrides.run_id ?? "run_1";

	return {
		baseline: {
			branch: "main",
			checks: [],
			expected_head_commit: "b".repeat(40),
			gates: [{ _tag: "required_checks_terminal" }],
			pull_request_native_id: "pr_7",
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pr_7",
				provider_id: "github",
				resource_kind: "pull_request",
			},
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
			review_decision: "review_required",
			reviews: [],
			review_threads: [],
		},
		...(overrides.maximum_generation === undefined
			? {}
			: { maximum_generation: overrides.maximum_generation }),
		owner: {
			_tag: "thread_run",
			agent_id: "agent_1",
			engine_id: "codex",
			run_id,
		},
		project_id: "project_1",
		request: {
			expected_head_commit: "b".repeat(40),
			gates: [{ _tag: "required_checks_terminal" }],
			pull_request_number: 7,
			source_run_id: run_id,
			workspace_id: "workspace_1",
		},
		request_fingerprint: overrides.request_fingerprint ?? "b".repeat(64),
		source_command: {
			message_id: overrides.command_id ?? "command_1",
			sent_at: overrides.sent_at ?? now,
		},
		target: {
			branch: "main",
			expected_head_commit: "b".repeat(40),
			pull_request_number: 7,
			pull_request_origin: {
				native_id: "pr_7",
				provider_id: "github",
				resource_kind: "pull_request",
			},
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
		},
		thread_id: overrides.thread_id ?? "thread_1",
		wait_id: overrides.wait_id ?? "wait_1",
	};
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

describe("ExternalWaitRepository", () => {
	it("replays the original result and keeps provider evidence out of public state", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					const accepted = yield* repository.Register(registration_for());
					const replay = yield* repository.Register(registration_for());
					const changed = yield* Effect.exit(
						repository.Register(
							registration_for({ request_fingerprint: "c".repeat(64) }),
						),
					);

					return {
						accepted,
						changed,
						query: yield* repository.Query({ thread_id: "thread_1" }),
						replay,
						runs: yield* database.client.select().from(OrchestrationRuns),
						waits: yield* database.client.select().from(ExternalWaits),
					};
				}),
			);

			expect(result.accepted.status).toBe("accepted");
			expect(result.replay).toEqual({ ...result.accepted, status: "duplicate" });
			expect(result.changed._tag).toBe("Failure");
			expect(result.query.snapshots[0]).not.toHaveProperty("baseline");
			expect(JSON.stringify(result.query)).not.toContain("pull_request_native_id");
			expect(result.waits[0]?.baseline_json).toContain("github.com");
			expect(result.waits[0]?.baseline_fingerprint).not.toBe(
				registration_for().request_fingerprint,
			);
			expect(result.runs[0]?.status).toBe("waiting_external");
		} finally {
			await instance.dispose();
		}
	});

	it("fences project deletion until its waits record a visible project-removed settlement", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const fenced = yield* Effect.exit(database.client.delete(Projects));
					const cancelled = yield* repository.Cancel({
						now: "2026-07-14T15:01:00.000Z",
						reason: "project_removed",
						source_command: {
							message_id: "project_remove_1",
							sent_at: "2026-07-14T15:01:00.000Z",
						},
						thread_id: "thread_1",
						wait_id: "wait_1",
					});
					const events = yield* database.client.select().from(JournalEvents);

					return { cancelled, events, fenced };
				}),
			);

			expect(result.fenced._tag).toBe("Failure");
			expect(Option.getOrThrow(result.cancelled).state).toEqual({
				_tag: "cancelled",
				reason: "project_removed",
			});
			expect(JSON.parse(result.events.at(-1)?.payload_json ?? "{}")).toMatchObject({
				snapshot: {
					state: { _tag: "cancelled", reason: "project_removed" },
					wait_id: "wait_1",
				},
				type: "external_wait.updated",
			});
		} finally {
			await instance.dispose();
		}
	});

	it("fails closed when a valid private baseline no longer matches its fingerprint", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;
					const registration = registration_for();

					yield* Seed;
					yield* repository.Register(registration);
					yield* database.client.update(ExternalWaits).set({
						baseline_json: JSON.stringify({
							...registration.baseline,
							review_decision: "approved",
						}),
					});

					return {
						claim: yield* Effect.exit(
							repository.ClaimObservation({
								lease_owner: "worker_1",
								now: "2026-07-14T15:01:00.000Z",
								wait_id: "wait_1",
							}),
						),
						query: yield* Effect.exit(repository.Query({ thread_id: "thread_1" })),
					};
				}),
			);

			expect(result.claim._tag).toBe("Failure");
			expect(result.query._tag).toBe("Failure");
		} finally {
			await instance.dispose();
		}
	});

	it("binds an assignment-owned wait to its active graph run and workspace", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;
					const base = registration_for({
						command_id: "graph_command_1",
						request_fingerprint: "d".repeat(64),
						run_id: "graph_run_1",
						wait_id: "graph_wait_1",
					});
					const registration: ExternalWaitRegistration = {
						...base,
						owner: {
							_tag: "assignment_run",
							agent_id: "graph_agent_1",
							assignment_id: "assignment_1",
							engine_id: "codex",
							group_id: "group_1",
							run_id: "graph_run_1",
						},
					};

					yield* Seed;
					yield* SeedGraphRun;
					const accepted = yield* repository.Register(registration);

					return {
						accepted,
						assignments: yield* database.client.select().from(Assignments),
						runs: yield* database.client.select().from(AgentRuns),
					};
				}),
			);

			expect(result.accepted.snapshot.owner._tag).toBe("assignment_run");
			expect(result.assignments[0]?.state).toBe("waiting_external");
			expect(result.runs[0]).toMatchObject({
				dispatch_status: "waiting_external",
				state: "waiting_external",
			});
		} finally {
			await instance.dispose();
		}
	});

	it("leases observations across owners and releases no-change checks without event churn", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const first = yield* repository.ClaimObservation({
						lease_owner: "worker_1",
						now: "2026-07-14T15:01:00.000Z",
						wait_id: "wait_1",
					});
					const blocked = yield* repository.ClaimObservation({
						lease_owner: "worker_2",
						now: "2026-07-14T15:01:00.000Z",
						wait_id: "wait_1",
					});
					const released = yield* repository.RecordObservation({
						lease_owner: "worker_1",
						next_observation_at: "2026-07-14T15:02:00.000Z",
						now: "2026-07-14T15:01:10.000Z",
						state: { _tag: "waiting" },
						wait_id: "wait_1",
					});
					const takeover = yield* repository.ClaimObservation({
						lease_owner: "worker_2",
						now: "2026-07-14T15:02:00.000Z",
						wait_id: "wait_1",
					});

					return {
						blocked,
						events: yield* database.client.select().from(JournalEvents),
						first,
						released,
						takeover,
						waits: yield* database.client.select().from(ExternalWaits),
					};
				}),
			);

			expect(Option.isSome(result.first)).toBe(true);
			expect(Option.isNone(result.blocked)).toBe(true);
			expect(Option.isSome(result.released)).toBe(true);
			expect(Option.isSome(result.takeover)).toBe(true);
			expect(result.events).toHaveLength(1);
			expect(result.waits[0]?.version).toBe(1);
			expect(result.waits[0]?.observer_lease_owner).toBe("worker_2");
		} finally {
			await instance.dispose();
		}
	});

	it("records a visible suspension only for the active observation lease", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					yield* repository.ClaimObservation({
						lease_owner: "worker_1",
						now: "2026-07-14T15:01:00.000Z",
						wait_id: "wait_1",
					});
					const stale = yield* Effect.exit(
						repository.RecordObservation({
							lease_owner: "worker_2",
							next_observation_at: "2026-07-14T15:02:00.000Z",
							now: "2026-07-14T15:01:10.000Z",
							state: { _tag: "suspended", reason: "provider_unavailable" },
							wait_id: "wait_1",
						}),
					);
					const suspended = yield* repository.RecordObservation({
						lease_owner: "worker_1",
						next_observation_at: "2026-07-14T15:02:00.000Z",
						now: "2026-07-14T15:01:10.000Z",
						state: { _tag: "suspended", reason: "provider_unavailable" },
						wait_id: "wait_1",
					});

					return {
						due: yield* repository.DiscoverDueObservations({
							now: "2026-07-14T16:00:00.000Z",
						}),
						events: yield* database.client.select().from(JournalEvents),
						stale,
						suspended,
					};
				}),
			);

			expect(result.stale._tag).toBe("Failure");
			expect(Option.getOrThrow(result.suspended).state).toEqual({
				_tag: "suspended",
				reason: "provider_unavailable",
			});
			expect(result.events).toHaveLength(2);
			expect(result.due).toEqual([]);
		} finally {
			await instance.dispose();
		}
	});

	it("gates wake delivery on source closure and settles one exact follow-up", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const wake = yield* repository.CreateWake({
						now: "2026-07-14T15:01:00.000Z",
						trigger: { _tag: "manual_resume" },
						wait_id: "wait_1",
					});
					const duplicate = yield* repository.CreateWake({
						now: "2026-07-14T15:01:01.000Z",
						trigger: {
							_tag: "review_changed",
							change_kind: "decision_changed",
							decision: "approved",
							total_reviews: 1,
							unresolved_thread_count: 0,
						},
						wait_id: "wait_1",
					});
					const before_close = yield* repository.DiscoverWakes({
						now: "2026-07-14T15:01:02.000Z",
					});

					yield* repository.MarkSourceClosed({
						now: "2026-07-14T15:01:02.000Z",
						wait_id: "wait_1",
					});
					const discovered = yield* repository.DiscoverWakes({
						now: "2026-07-14T15:01:03.000Z",
					});
					const first_claim = yield* repository.ClaimWake({
						lease_owner: "dispatcher_1",
						now: "2026-07-14T15:01:03.000Z",
						outbox_id: wake.outbox_id,
					});
					const blocked_claim = yield* repository.ClaimWake({
						lease_owner: "dispatcher_2",
						now: "2026-07-14T15:01:03.000Z",
						outbox_id: wake.outbox_id,
					});

					yield* repository.ReleaseWake({
						lease_owner: "dispatcher_1",
						now: "2026-07-14T15:01:10.000Z",
						outbox_id: wake.outbox_id,
					});
					const second_claim = yield* repository.ClaimWake({
						lease_owner: "dispatcher_2",
						now: "2026-07-14T15:01:11.000Z",
						outbox_id: wake.outbox_id,
					});
					const settled = yield* repository.SettleWake({
						follow_up_run_id: wake.follow_up_run_id,
						lease_owner: "dispatcher_2",
						mode: "native_resume",
						now: "2026-07-14T15:01:12.000Z",
						outbox_id: wake.outbox_id,
					});
					const replay = yield* repository.SettleWake({
						follow_up_run_id: wake.follow_up_run_id,
						lease_owner: "different_after_settlement",
						mode: "native_resume",
						now: "2026-07-14T16:00:00.000Z",
						outbox_id: wake.outbox_id,
					});
					const changed = yield* Effect.exit(
						repository.SettleWake({
							follow_up_run_id: wake.follow_up_run_id,
							lease_owner: "dispatcher_2",
							mode: "linked_run",
							now: "2026-07-14T16:00:00.000Z",
							outbox_id: wake.outbox_id,
						}),
					);

					return {
						before_close,
						blocked_claim,
						changed,
						discovered,
						duplicate,
						first_claim,
						replay,
						second_claim,
						settled,
						wake,
					};
				}),
			);

			expect(result.duplicate).toEqual(result.wake);
			expect(result.before_close).toEqual([]);
			expect(result.discovered).toEqual([result.wake.outbox_id]);
			expect(Option.isSome(result.first_claim)).toBe(true);
			expect(Option.isNone(result.blocked_claim)).toBe(true);
			expect(Option.isSome(result.second_claim)).toBe(true);
			expect(result.replay).toEqual(result.settled);
			expect(Option.getOrThrow(result.settled).state).toMatchObject({
				_tag: "woken",
				follow_up_run_id: result.wake.follow_up_run_id,
				mode: "native_resume",
			});
			expect(result.changed._tag).toBe("Failure");
		} finally {
			await instance.dispose();
		}
	});

	it("cancellation atomically fences a claimed wake and replays its original snapshot", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const wake = yield* repository.CreateWake({
						now: "2026-07-14T15:01:00.000Z",
						trigger: { _tag: "manual_resume" },
						wait_id: "wait_1",
					});
					yield* repository.MarkSourceClosed({
						now: "2026-07-14T15:01:01.000Z",
						wait_id: "wait_1",
					});
					yield* repository.ClaimWake({
						lease_owner: "dispatcher_1",
						now: "2026-07-14T15:01:02.000Z",
						outbox_id: wake.outbox_id,
					});
					const cancel_input = {
						now: "2026-07-14T15:01:03.000Z",
						reason: "user" as const,
						source_command: {
							message_id: "cancel_1",
							sent_at: "2026-07-14T15:01:03.000Z",
						},
						thread_id: "thread_1",
						wait_id: "wait_1",
					};
					const cancelled = yield* repository.Cancel(cancel_input);
					const replay = yield* repository.Cancel(cancel_input);
					const changed = yield* Effect.exit(
						repository.Cancel({ ...cancel_input, reason: "superseded" }),
					);
					const settle = yield* Effect.exit(
						repository.SettleWake({
							follow_up_run_id: wake.follow_up_run_id,
							lease_owner: "dispatcher_1",
							mode: "native_resume",
							now: "2026-07-14T15:01:04.000Z",
							outbox_id: wake.outbox_id,
						}),
					);

					return {
						cancelled,
						changed,
						discovered: yield* repository.DiscoverWakes({
							now: "2026-07-14T16:00:00.000Z",
						}),
						events: yield* database.client.select().from(JournalEvents),
						operations: yield* database.client.select().from(ExternalWaitOperations),
						outbox: yield* database.client.select().from(ExternalWaitWakeOutbox),
						replay,
						settle,
					};
				}),
			);

			expect(result.replay).toEqual(result.cancelled);
			expect(Option.getOrThrow(result.cancelled).state).toEqual({
				_tag: "cancelled",
				reason: "user",
			});
			expect(result.changed._tag).toBe("Failure");
			expect(result.settle._tag).toBe("Failure");
			expect(result.discovered).toEqual([]);
			expect(result.outbox[0]?.state).toBe("cancelled");
			expect(result.events).toHaveLength(3);
			expect(result.operations).toHaveLength(2);
		} finally {
			await instance.dispose();
		}
	});

	it("manual resume stays thread-bound and has exact command replay", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const source_command = {
						message_id: "manual_1",
						sent_at: "2026-07-14T15:01:00.000Z",
					};
					const crossed = yield* Effect.exit(
						repository.ManualResume({
							now: "2026-07-14T15:01:00.000Z",
							source_command,
							thread_id: "thread_2",
							wait_id: "wait_1",
						}),
					);
					const wake = yield* repository.ManualResume({
						now: "2026-07-14T15:01:00.000Z",
						source_command,
						thread_id: "thread_1",
						wait_id: "wait_1",
					});
					const replay = yield* repository.ManualResume({
						now: "2026-07-14T15:01:01.000Z",
						source_command,
						thread_id: "thread_1",
						wait_id: "wait_1",
					});

					return { crossed, replay, wake };
				}),
			);

			expect(result.crossed._tag).toBe("Failure");
			expect(result.replay).toEqual(result.wake);
		} finally {
			await instance.dispose();
		}
	});

	it("derives retry generations only from settled follow-ups and exhausts the bound", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for({ maximum_generation: 1 }));
					const wake = yield* repository.CreateWake({
						now: "2026-07-14T15:01:00.000Z",
						trigger: { _tag: "manual_resume" },
						wait_id: "wait_1",
					});
					yield* repository.MarkSourceClosed({
						now: "2026-07-14T15:01:01.000Z",
						wait_id: "wait_1",
					});
					yield* repository.ClaimWake({
						lease_owner: "dispatcher_1",
						now: "2026-07-14T15:01:02.000Z",
						outbox_id: wake.outbox_id,
					});
					yield* repository.SettleWake({
						follow_up_run_id: wake.follow_up_run_id,
						lease_owner: "dispatcher_1",
						mode: "linked_run",
						now: "2026-07-14T15:01:03.000Z",
						outbox_id: wake.outbox_id,
					});
					yield* SeedRun(wake.follow_up_run_id);

					const exhausted = yield* repository.Register(
						registration_for({
							command_id: "command_2",
							request_fingerprint: "c".repeat(64),
							run_id: wake.follow_up_run_id,
							sent_at: "2026-07-14T15:02:00.000Z",
							wait_id: "wait_2",
						}),
					);

					return {
						exhausted,
						runs: yield* database.client.select().from(OrchestrationRuns),
					};
				}),
			);

			expect(result.exhausted.snapshot.state).toEqual({ _tag: "exhausted" });
			expect(result.exhausted.snapshot.generation).toBe(1);
			expect(result.exhausted.snapshot.maximum_generation).toBe(1);
			expect(
				result.runs.find((run) => run.run_id === result.exhausted.snapshot.owner.run_id)
					?.status,
			).toBe("running");
		} finally {
			await instance.dispose();
		}
	});

	it("bounds stable query and due-observation discovery to 64 rows", async () => {
		const instance = runtime(await Effect.runPromise(MakeDatabasePath));

		try {
			const result = await instance.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for({ wait_id: "wait_000" }));
					const [template] = yield* database.client.select().from(ExternalWaits).limit(1);

					if (!template) {
						return yield* Effect.die(new Error("Expected registered wait template"));
					}

					const wait_rows = Array.from({ length: 64 }, (_, offset) => {
						const index = offset + 1;
						const suffix = index.toString().padStart(3, "0");

						return {
							...template,
							source_run_id: `source_${suffix}`,
							wait_id: `wait_${suffix}`,
						};
					});

					yield* Effect.forEach(
						wait_rows,
						(row) => database.client.insert(ExternalWaits).values(row),
						{ concurrency: 1, discard: true },
					);

					return {
						due: yield* repository.DiscoverDueObservations({
							now: "2026-07-14T15:01:00.000Z",
						}),
						query: yield* repository.Query({ thread_id: "thread_1" }),
					};
				}),
			);

			expect(result.query.snapshots).toHaveLength(64);
			expect(result.query.truncated).toBe(true);
			expect(result.query.snapshots[0]?.wait_id).toBe("wait_000");
			expect(result.query.snapshots.at(-1)?.wait_id).toBe("wait_063");
			expect(result.due).toHaveLength(64);
			expect(result.due.at(-1)).toBe("wait_063");
		} finally {
			await instance.dispose();
		}
	});

	it("allows only one observation owner across two backend runtimes", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = runtime(database_path);
		const second_runtime = runtime(database_path);

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
				}),
			);
			await second_runtime.runPromise(
				Effect.flatMap(ExternalWaitRepository, (repository) =>
					repository.Query({ thread_id: "thread_1" }),
				),
			);

			const claims = await Promise.all([
				first_runtime.runPromise(
					Effect.flatMap(ExternalWaitRepository, (repository) =>
						repository.ClaimObservation({
							lease_owner: "runtime_1",
							now: "2026-07-14T15:01:00.000Z",
							wait_id: "wait_1",
						}),
					),
				),
				second_runtime.runPromise(
					Effect.flatMap(ExternalWaitRepository, (repository) =>
						repository.ClaimObservation({
							lease_owner: "runtime_2",
							now: "2026-07-14T15:01:00.000Z",
							wait_id: "wait_1",
						}),
					),
				),
			]);

			expect(claims.filter(Option.isSome)).toHaveLength(1);
			expect(claims.filter(Option.isNone)).toHaveLength(1);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("rediscovers a pending wake after the creating runtime restarts", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = runtime(database_path);
		let outbox_id = "";

		try {
			outbox_id = await first_runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* ExternalWaitRepository;

					yield* Seed;
					yield* repository.Register(registration_for());
					const wake = yield* repository.CreateWake({
						now: "2026-07-14T15:01:00.000Z",
						trigger: { _tag: "manual_resume" },
						wait_id: "wait_1",
					});
					yield* repository.MarkSourceClosed({
						now: "2026-07-14T15:01:01.000Z",
						wait_id: "wait_1",
					});

					return wake.outbox_id;
				}),
			);
		} finally {
			await first_runtime.dispose();
		}

		const restarted = runtime(database_path);

		try {
			const result = await restarted.runPromise(
				Effect.gen(function* () {
					const repository = yield* ExternalWaitRepository;
					const discovered = yield* repository.DiscoverWakes({
						now: "2026-07-14T15:01:02.000Z",
					});

					return {
						claim: yield* repository.ClaimWake({
							lease_owner: "restarted_dispatcher",
							now: "2026-07-14T15:01:02.000Z",
							outbox_id,
						}),
						discovered,
					};
				}),
			);

			expect(result.discovered).toEqual([outbox_id]);
			expect(Option.isSome(result.claim)).toBe(true);
		} finally {
			await restarted.dispose();
		}
	});
});
