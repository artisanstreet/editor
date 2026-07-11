import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Queue, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	type CommandEnvelope,
	type HelloEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
} from "@artisan/protocol";
import {
	make_backend_runtime,
	ProtocolRouter,
	ProtocolServer,
	type ProtocolConnection,
	ThreadErasure,
	ThreadRetention,
	ThreadRetentionClock,
	ThreadRetentionScheduler,
	ThreadResourceQuiescer,
	ThreadResourceQuiescenceFailure,
} from "@artisan/backend";
import { Database } from "../../modules/backend/src/persistence/database";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	AgentRuns,
	EventStreams,
	JournalCommands,
	JournalEvents,
	OrchestrationArtifacts,
	OrchestrationGroups,
	OrchestrationRawObservations,
	TerminalCommands,
	TerminalSessions,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../../modules/backend/src/persistence/schema";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-thread-retention-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: { value: string } = { value: "2026-07-10T18:00:00.000Z" }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_thread_retention_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

function make_retention_clock(now: { value: string }) {
	return Layer.succeed(ThreadRetentionClock, {
		Now: Effect.sync(() => now.value),
	});
}

function make_inert_scheduler(active: { value: number }) {
	return Layer.succeed(ThreadRetentionScheduler, {
		Schedule: () =>
			Effect.acquireRelease(
				Effect.sync(() => {
					active.value += 1;
				}),
				() =>
					Effect.sync(() => {
						active.value -= 1;
					}),
			).pipe(Effect.andThen(Effect.never)),
	});
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_1",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_query(message_id: string): ThreadRetentionQueryEnvelope {
	return {
		kind: "thread.retention.query",
		message_id,
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_update(
	message_id: string,
	enabled: boolean,
	inactivity_days: number,
): ThreadRetentionUpdateEnvelope {
	return {
		kind: "thread.retention.update",
		message_id,
		origin: "frontend",
		payload: {
			enabled,
			inactivity_days,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-10T18:00:00.000Z",
	};
}

function make_create(message_id = "create_1", thread_id = "thread_expired"): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload: { title: "Expired durable content", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-01T18:00:00.000Z",
		thread_id,
	};
}

const UpdatePolicy = (
	connection: ProtocolConnection,
	message_id: string,
	enabled: boolean,
	inactivity_days: number,
) =>
	Effect.gen(function* () {
		yield* connection.Receive(make_update(message_id, enabled, inactivity_days));

		return yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
	});

const OpenRetentionConnection = Effect.gen(function* () {
	const server = yield* ProtocolServer;
	const connection = yield* server.Open;

	yield* connection.Receive(make_hello());
	yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);

	return connection;
});

async function make_controlled_scheduler(active: { value: number }) {
	const ticks = await Effect.runPromise(Queue.unbounded<void>());
	const completed = await Effect.runPromise(Queue.unbounded<void>());
	const layer = Layer.succeed(ThreadRetentionScheduler, {
		Schedule: (task) =>
			Effect.acquireRelease(
				Effect.sync(() => {
					active.value += 1;
				}),
				() =>
					Effect.sync(() => {
						active.value -= 1;
					}),
			).pipe(
				Effect.andThen(
					Effect.forever(
						Queue.take(ticks).pipe(
							Effect.andThen(task),
							Effect.andThen(Queue.offer(completed, undefined)),
						),
					),
				),
			),
	});

	return { completed, layer, ticks };
}

afterEach(async () => {
	await Promise.all(
		temporary_directories.splice(0).map((directory) =>
			rm(directory, {
				force: true,
				recursive: true,
			}),
		),
	);
});

describe("thread retention", () => {
	it("reads the default policy and durably updates it with exact retries", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);
						yield* connection.Receive(make_query("query_default"));
						const [initial] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);
						const update = make_update("retention_update_1", false, 30);
						yield* connection.Receive(update);
						const first = yield* connection.Outbound.pipe(
							Stream.take(2),
							Stream.runCollect,
						);
						yield* connection.Receive(update);
						const retry = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);
						yield* connection.Receive(make_update("retention_update_1", true, 14));
						const [conflict] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						yield* connection.Receive(make_query("query_updated"));
						const [updated] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						return { conflict, first, initial, retry, updated };
					}),
				),
			);

			expect(result.initial).toMatchObject({
				kind: "thread.retention.query.result",
				payload: { enabled: true, inactivity_days: 7 },
			});
			expect(result.first[0]).toMatchObject({
				payload: { status: "accepted" },
			});
			expect(result.retry[0]).toMatchObject({
				payload: { status: "duplicate" },
			});
			expect(result.conflict).toMatchObject({
				payload: {
					error: { code: "command.id_conflict" },
					status: "rejected",
				},
			});
			expect(result.updated).toMatchObject({
				kind: "thread.retention.query.result",
				payload: { enabled: false, inactivity_days: 30 },
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("deeply erases content while preserving a contiguous content-free ledger", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const erasure = yield* ThreadErasure;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create());
					yield* database.client.insert(OrchestrationGroups).values({
						coordinator_agent_id: "agent_1",
						created_at: "2026-07-01T18:00:00.000Z",
						group_id: "group_1",
						journal_sequence: 1,
						max_concurrency: 1,
						state: "complete",
						thread_id: "thread_expired",
						updated_at: "2026-07-01T18:00:00.000Z",
						version: 1,
					});
					yield* database.client.insert(AgentRuns).values({
						agent_id: "agent_1",
						assignment_id: "assignment_1",
						attempt: 1,
						created_at: "2026-07-01T18:00:00.000Z",
						dispatch_status: "completed",
						engine_id: "engine_1",
						group_id: "group_1",
						last_observation_sequence: 1,
						profile: "default",
						run_id: "graph_run_1",
						state: "complete",
						updated_at: "2026-07-01T18:00:00.000Z",
					});
					yield* database.client.insert(OrchestrationRawObservations).values({
						engine_id: "engine_1",
						frame_json: JSON.stringify({ secret: "thread content" }),
						observation_id: "observation_1",
						run_id: "graph_run_1",
						sequence: 1,
						transport: "test",
					});
					yield* database.client.insert(OrchestrationArtifacts).values({
						artifact_id: "artifact_1",
						assignment_id: "assignment_1",
						content: "private diff content",
						created_at: "2026-07-01T18:00:00.000Z",
						group_id: "group_1",
						kind: "diff",
						label: "Sensitive diff",
						run_id: "graph_run_1",
					});
					yield* database.client.insert(TerminalSessions).values({
						args_json: "[]",
						cols: 80,
						created_at: "2026-07-01T18:00:00.000Z",
						executable: "pwsh",
						generation: 1,
						owner_instance_id: "old_backend",
						rows: 24,
						state: "closed",
						terminal_id: "terminal_1",
						thread_id: "thread_expired",
						updated_at: "2026-07-01T18:00:00.000Z",
						workspace_id: "workspace_1",
						working_directory: "C:/workspace",
					});
					yield* database.client.insert(TerminalCommands).values({
						claimed_session_json: "{}",
						created_at: "2026-07-01T18:00:00.000Z",
						generation: 1,
						message_id: "terminal_command_1",
						payload_json: "{}",
						status: "completed",
						terminal_id: "terminal_1",
						updated_at: "2026-07-01T18:00:00.000Z",
					});

					const erased = yield* erasure.CleanupExpired(
						"2026-07-08T18:00:00.000Z",
						"2026-07-10T18:00:00.000Z",
					);
					const repeated = yield* erasure.ResumeClaimed("2026-07-10T18:00:00.000Z");
					const replay = yield* journal.ReadReplay({ after_journal_sequence: 0 });
					const recreate = yield* router.Route(make_create("create_after_erasure"));
					const snapshot = yield* threads.Snapshot();
					const counts = yield* Effect.all({
						artifacts: database.client.select().from(OrchestrationArtifacts),
						claims: database.client.select().from(ThreadErasureClaims),
						commands: database.client.select().from(JournalCommands),
						events: database.client.select().from(JournalEvents),
						event_streams: database.client.select().from(EventStreams),
						groups: database.client.select().from(OrchestrationGroups),
						raw: database.client.select().from(OrchestrationRawObservations),
						runs: database.client.select().from(AgentRuns),
						terminal_commands: database.client.select().from(TerminalCommands),
						terminals: database.client.select().from(TerminalSessions),
						threads: database.client.select().from(Threads),
						tombstones: database.client.select().from(ThreadTombstones),
					});

					return { counts, erased, recreate, repeated, replay, snapshot };
				}),
			);

			expect(result.erased).toEqual(["thread_expired"]);
			expect(result.repeated).toEqual([]);
			expect(result.replay.map((event) => event.payload)).toEqual([
				{ type: "thread.content_erased" },
				{ type: "thread.erased" },
			]);
			expect(result.replay.map((event) => event.journal_sequence)).toEqual([1, 2]);
			expect(result.replay.map((event) => event.sequence)).toEqual([1, 2]);
			expect(JSON.stringify(result.replay)).not.toContain("Expired thread");
			expect(result.recreate[0]).toMatchObject({
				payload: { status: "rejected" },
			});
			expect(result.snapshot.threads).toEqual([]);
			expect(result.counts).toMatchObject({
				artifacts: [],
				claims: [],
				commands: [],
				event_streams: [{ last_sequence: 2, stream_id: "thread:thread_expired" }],
				events: [
					{
						agent_id: null,
						causation_id: "thread_content_erased_1",
						correlation_id: "thread_content_erased_1",
						event_id: "thread_content_erased_1",
						event_type: "thread.content_erased",
						occurred_at: "1970-01-01T00:00:00.000Z",
						origin: "backend",
						payload_json: '{"type":"thread.content_erased"}',
						raw_origin_json: null,
						run_id: null,
						schema_version: 1,
						sequence: 1,
						stream_id: "thread:thread_expired",
						stream_sequence: 1,
						thread_id: "thread_expired",
					},
					{
						agent_id: null,
						causation_id: "thread_erased_thread_expired",
						correlation_id: "thread_erased_thread_expired",
						event_id: "thread_erased_thread_expired",
						event_type: "thread.erased",
						occurred_at: "2026-07-10T18:00:00.000Z",
						origin: "backend",
						payload_json: '{"type":"thread.erased"}',
						raw_origin_json: null,
						run_id: null,
						schema_version: 1,
						sequence: 2,
						stream_id: "thread:thread_expired",
						stream_sequence: 2,
						thread_id: "thread_expired",
					},
				],
				groups: [],
				raw: [],
				runs: [],
				terminal_commands: [],
				terminals: [],
				threads: [],
				tombstones: [
					{
						deleted_at: "2026-07-10T18:00:00.000Z",
						thread_id: "thread_expired",
					},
				],
			});
		} finally {
			await runtime.dispose();
		}
	});

	it("runs default seven-day cleanup on startup and releases its scheduler", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const active_schedulers = { value: 0 };
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler(active_schedulers),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			await first_runtime.runPromise(
				Effect.gen(function* () {
					const router = yield* ProtocolRouter;

					yield* router.Route(make_create());
				}),
			);
			expect(active_schedulers.value).toBe(1);
		} finally {
			await first_runtime.dispose();
		}

		expect(active_schedulers.value).toBe(0);
		now.value = "2026-07-09T18:00:00.000Z";
		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler(active_schedulers),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const snapshot = await second_runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadReadModel).Snapshot();
				}),
			);

			expect(snapshot.threads).toEqual([]);
			expect(active_schedulers.value).toBe(1);
		} finally {
			await second_runtime.dispose();
		}

		expect(active_schedulers.value).toBe(0);
	});

	it("honors disabled and custom policies while exempting pinned threads", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenRetentionConnection;
						const retention = yield* ThreadRetention;
						const router = yield* ProtocolRouter;
						const threads = yield* ThreadReadModel;

						yield* router.Route(make_create("create_due", "thread_due"));
						yield* router.Route(make_create("create_pinned", "thread_pinned"));
						yield* router.Route(make_create("create_archived", "thread_archived"));
						yield* router.Route({
							...make_create("pin_1", "thread_pinned"),
							payload: { type: "thread.pin" },
						});
						yield* router.Route({
							...make_create("archive_1", "thread_archived"),
							payload: { type: "thread.archive" },
						});
						yield* UpdatePolicy(connection, "disable_1", false, 10);
						now.value = "2026-07-20T18:00:00.000Z";
						const disabled = yield* retention.RunCleanup;
						const before_enabled = yield* threads.Snapshot();
						yield* UpdatePolicy(connection, "enable_1", true, 10);
						const enabled = yield* retention.RunCleanup;
						const after_enabled = yield* threads.Snapshot();

						return { after_enabled, before_enabled, disabled, enabled };
					}),
				),
			);

			expect(result.disabled).toEqual([]);
			expect(result.before_enabled.threads).toHaveLength(3);
			expect([...result.enabled].sort()).toEqual(["thread_archived", "thread_due"]);
			expect(result.after_enabled.threads).toMatchObject([
				{ pinned: true, thread_id: "thread_pinned" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("runs periodic cleanup from a deterministic scheduler without leaking its fiber", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const active_schedulers = { value: 0 };
		const scheduler = await make_controlled_scheduler(active_schedulers);
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: scheduler.layer,
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			await runtime.runPromise(
				Effect.gen(function* () {
					yield* ThreadRetention;
					yield* (yield* ProtocolRouter).Route(
						make_create("periodic_create", "thread_periodic"),
					);
				}),
			);
			now.value = "2026-07-09T18:00:00.000Z";
			await Effect.runPromise(Queue.offer(scheduler.ticks, undefined));
			await Effect.runPromise(Queue.take(scheduler.completed));

			const snapshot = await runtime.runPromise(
				Effect.gen(function* () {
					return yield* (yield* ThreadReadModel).Snapshot();
				}),
			);

			expect(snapshot.threads).toEqual([]);
			expect(active_schedulers.value).toBe(1);
		} finally {
			await runtime.dispose();
		}

		expect(active_schedulers.value).toBe(0);
	});

	it("resumes a durable erasure claim after restart even when retention is disabled", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			await first_runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const connection = yield* OpenRetentionConnection;
						const database = yield* Database;
						const router = yield* ProtocolRouter;

						yield* router.Route(make_create("claimed_create", "thread_claimed"));
						yield* UpdatePolicy(connection, "disable_claimed", false, 7);
						yield* database.client.insert(ThreadErasureClaims).values({
							claimed_at: "2026-07-09T18:00:00.000Z",
							thread_id: "thread_claimed",
						});
					}),
				),
			);
		} finally {
			await first_runtime.dispose();
		}

		now.value = "2026-07-10T18:00:00.000Z";
		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await second_runtime.runPromise(
				Effect.gen(function* () {
					yield* ThreadRetention;
					const database = yield* Database;

					return {
						claims: yield* database.client.select().from(ThreadErasureClaims),
						threads: (yield* (yield* ThreadReadModel).Snapshot()).threads,
						tombstones: yield* database.client.select().from(ThreadTombstones),
					};
				}),
			);

			expect(result.claims).toEqual([]);
			expect(result.threads).toEqual([]);
			expect(result.tombstones).toMatchObject([{ thread_id: "thread_claimed" }]);
		} finally {
			await second_runtime.dispose();
		}
	});

	it("resets the retention cutoff after meaningful activity", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const retention = yield* ThreadRetention;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(make_create("activity_create", "thread_active"));
					now.value = "2026-07-06T18:00:00.000Z";
					yield* router.Route({
						...make_create("activity_reset", "thread_active"),
						payload: {
							activity_kind: "file_attached",
							type: "thread.activity.record",
						},
					});
					now.value = "2026-07-09T18:00:00.000Z";
					const first_cleanup = yield* retention.RunCleanup;
					const after_first = yield* threads.Snapshot();
					now.value = "2026-07-14T18:00:00.000Z";
					const second_cleanup = yield* retention.RunCleanup;

					return {
						after_first,
						final: yield* threads.Snapshot(),
						first_cleanup,
						second_cleanup,
					};
				}),
			);

			expect(result.first_cleanup).toEqual([]);
			expect(result.after_first.threads).toMatchObject([
				{
					last_activity_at: "2026-07-06T18:00:00.000Z",
					thread_id: "thread_active",
				},
			]);
			expect(result.second_cleanup).toEqual(["thread_active"]);
			expect(result.final.threads).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});

	it("recovers a persisted claim after quiescence fails before erasure", async () => {
		const database_path = await make_database_path();
		const now = { value: "2026-07-01T18:00:00.000Z" };
		const failing_quiescer = Layer.succeed(ThreadResourceQuiescer, {
			Quiesce: (thread_id) =>
				Effect.fail(
					new ThreadResourceQuiescenceFailure({
						cause: new Error(`failed to quiesce ${thread_id}`),
					}),
				),
		});
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
			thread_resource_quiescer: failing_quiescer,
		});

		try {
			const failed = await first_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const retention = yield* ThreadRetention;
					const router = yield* ProtocolRouter;

					yield* router.Route(make_create("failed_create", "thread_failed"));
					now.value = "2026-07-09T18:00:00.000Z";
					const failure = yield* retention.RunCleanup.pipe(Effect.flip);

					return {
						claims: yield* database.client.select().from(ThreadErasureClaims),
						failure,
						threads: (yield* (yield* ThreadReadModel).Snapshot()).threads,
					};
				}),
			);

			expect(failed.failure).toBeInstanceOf(Error);
			expect(failed.claims).toMatchObject([{ thread_id: "thread_failed" }]);
			expect(failed.threads).toMatchObject([{ thread_id: "thread_failed" }]);
		} finally {
			await first_runtime.dispose();
		}

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			retention_clock: make_retention_clock(now),
			retention_scheduler: make_inert_scheduler({ value: 0 }),
			runtime_metadata: make_metadata_layer(now),
		});

		try {
			const recovered = await second_runtime.runPromise(
				Effect.gen(function* () {
					yield* ThreadRetention;
					const database = yield* Database;

					return {
						claims: yield* database.client.select().from(ThreadErasureClaims),
						threads: (yield* (yield* ThreadReadModel).Snapshot()).threads,
						tombstones: yield* database.client.select().from(ThreadTombstones),
					};
				}),
			);

			expect(recovered.claims).toEqual([]);
			expect(recovered.threads).toEqual([]);
			expect(recovered.tombstones).toMatchObject([{ thread_id: "thread_failed" }]);
		} finally {
			await second_runtime.dispose();
		}
	});
});
