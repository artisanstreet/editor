import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { make_backend_runtime, ProjectionRebuildService, ProtocolRouter } from "@artisan/backend";
import type { CommandEnvelope } from "@artisan/protocol";

import { Database } from "../../modules/backend/src/persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
} from "../../modules/backend/src/persistence/tables";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const acknowledgement_migration_path = fileURLToPath(
	new URL(
		"../../modules/backend/drizzle/20260810203620_daily_mojo/migration.sql",
		import.meta.url,
	),
);
const directories: Array<string> = [];

const MakeRuntimeMetadata = () => {
	let id = 0;
	let instant = Date.parse("2026-08-10T20:00:00.000Z");

	return Layer.succeed(
		RuntimeMetadata,
		RuntimeMetadata.of({
			instance_id: "thread-attention-acknowledgement-test",
			MakeId: (prefix) => Effect.sync(() => `${prefix}_${(id += 1)}`),
			Now: Effect.sync(() => new Date((instant += 1_000)).toISOString()),
		}),
	);
};

const Command = (message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope => ({
	kind: "command",
	message_id,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-10T20:00:00.000Z",
	thread_id: "thread_attention",
});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread attention acknowledgement", () => {
	it("migrates legacy failures into replayable acknowledgement facts", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-thread-attention-migration-"));
		directories.push(directory);
		const database = new DatabaseSync(join(directory, "artisan.db"));

		try {
			database.exec(`
				CREATE TABLE threads (
					thread_id text PRIMARY KEY,
					reader_activity_at text NOT NULL,
					live_status text NOT NULL,
					updated_at text NOT NULL
				);
				CREATE TABLE event_streams (
					stream_id text PRIMARY KEY,
					last_sequence integer NOT NULL
				);
				CREATE TABLE journal_events (
					sequence integer PRIMARY KEY AUTOINCREMENT,
					stream_id text NOT NULL,
					stream_sequence integer NOT NULL,
					schema_version integer NOT NULL,
					event_id text NOT NULL UNIQUE,
					idempotency_key text UNIQUE,
					correlation_id text NOT NULL,
					causation_id text NOT NULL,
					origin text NOT NULL,
					raw_origin_json text,
					event_type text NOT NULL,
					thread_id text NOT NULL,
					run_id text,
					agent_id text,
					payload_json text NOT NULL,
					occurred_at text NOT NULL,
					UNIQUE (stream_id, stream_sequence)
				);
				INSERT INTO threads VALUES
					('failed-thread', '2026-08-10T20:00:00.000Z', 'Failed to complete', '2026-08-10T20:00:01.000Z'),
					('idle-thread', '2026-08-10T20:00:02.000Z', 'Idle', '2026-08-10T20:00:03.000Z');
				INSERT INTO event_streams VALUES
					('thread:failed-thread', 7),
					('thread:idle-thread', 3);
			`);
			database.exec(await readFile(acknowledgement_migration_path, "utf8"));

			const failed = database
				.prepare(
					"SELECT reader_acknowledged_activity_at FROM threads WHERE thread_id = 'failed-thread'",
				)
				.get() as { readonly reader_acknowledged_activity_at: string };
			const idle = database
				.prepare(
					"SELECT reader_acknowledged_activity_at FROM threads WHERE thread_id = 'idle-thread'",
				)
				.get() as { readonly reader_acknowledged_activity_at: string };
			const event = database
				.prepare(
					"SELECT event_type, payload_json, stream_sequence FROM journal_events WHERE thread_id = 'failed-thread'",
				)
				.get() as {
				readonly event_type: string;
				readonly payload_json: string;
				readonly stream_sequence: number;
			};
			const streams = database
				.prepare("SELECT stream_id, last_sequence FROM event_streams ORDER BY stream_id")
				.all();

			expect(failed.reader_acknowledged_activity_at).toBe("2026-08-10T20:00:00.000Z");
			expect(idle.reader_acknowledged_activity_at).toBe("1970-01-01T00:00:00.000Z");
			expect(event).toMatchObject({
				event_type: "thread.attention.acknowledged",
				stream_sequence: 8,
			});
			expect(JSON.parse(event.payload_json)).toEqual({
				reader_activity_at: "2026-08-10T20:00:00.000Z",
				type: "thread.attention.acknowledged",
			});
			expect(streams).toEqual([
				{ last_sequence: 8, stream_id: "thread:failed-thread" },
				{ last_sequence: 3, stream_id: "thread:idle-thread" },
			]);
		} finally {
			database.close();
		}
	});

	it("preserves a legacy backfill acknowledgement through projection rebuild", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-thread-attention-backfill-"));
		directories.push(directory);
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			runtime_metadata: MakeRuntimeMetadata(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						Command("legacy-create", {
							title: "Legacy attention",
							type: "thread.create",
						}),
					);
					yield* router.Route(
						Command("legacy-failure", {
							activity_kind: "run_failed",
							type: "thread.activity.record",
						}),
					);
					const before = (yield* threads.Snapshot()).threads[0]!;
					const stream_id = `thread:${before.thread_id}`;
					const stream = (yield* database.client.select().from(EventStreams)).find(
						(candidate) => candidate.stream_id === stream_id,
					);
					if (stream === undefined) throw new Error("thread stream missing");
					const stream_sequence = stream.last_sequence + 1;
					yield* database.client.insert(JournalEvents).values({
						causation_id: "migration:attention:legacy",
						correlation_id: "migration:attention:legacy",
						event_id: "migration:attention:legacy:event",
						event_type: "thread.attention.acknowledged",
						idempotency_key: "migration:attention:legacy",
						occurred_at: before.updated_at,
						origin: "backend",
						payload_json: JSON.stringify({
							reader_activity_at: before.reader_activity_at,
							type: "thread.attention.acknowledged",
						}),
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: before.thread_id,
					});
					yield* database.client.run(
						`UPDATE event_streams SET last_sequence = ${stream_sequence} WHERE stream_id = '${stream_id}'`,
					);
					/** Force the projection stale: replay must restore the migration fact. */
					yield* database.client.run(
						`UPDATE threads SET reader_acknowledged_activity_at = '1970-01-01T00:00:00.000Z' WHERE thread_id = '${before.thread_id}'`,
					);

					yield* rebuild.Rebuild();
					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);

			expect(result.reader_acknowledged_activity_at).toBe(result.reader_activity_at);
		} finally {
			await runtime.dispose();
		}
	});

	it("persists exact acknowledgements, preserves them through rebuild and restart, and cannot acknowledge unseen activity", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-thread-attention-"));
		directories.push(directory);
		const database_path = join(directory, "artisan.db");
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: MakeRuntimeMetadata(),
		});
		let persisted_acknowledged_activity_at: string | undefined;

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const rebuild = yield* ProjectionRebuildService;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						Command("create", { title: "Attention", type: "thread.create" }),
					);
					yield* router.Route(
						Command("visible-activity", {
							activity_kind: "run_failed",
							type: "thread.activity.record",
						}),
					);
					const before = (yield* threads.Snapshot()).threads[0]!;
					const acknowledge = Command("acknowledge", {
						reader_activity_at: before.reader_activity_at!,
						type: "thread.attention.acknowledge",
					});
					const first = yield* router.Route(acknowledge);
					const duplicate = yield* router.Route(acknowledge);
					const acknowledged = (yield* threads.Snapshot()).threads[0]!;

					yield* router.Route(
						Command("new-visible-activity", {
							activity_kind: "run_completed",
							type: "thread.activity.record",
						}),
					);
					const after_activity = (yield* threads.Snapshot()).threads[0]!;
					yield* router.Route(
						Command("stale-acknowledge", {
							reader_activity_at: before.reader_activity_at!,
							type: "thread.attention.acknowledge",
						}),
					);
					const stale_acknowledged = (yield* threads.Snapshot()).threads[0]!;
					const future = yield* router
						.Route(
							Command("future-acknowledge", {
								reader_activity_at: "2026-08-11T20:00:00.000Z",
								type: "thread.attention.acknowledge",
							}),
						)
						.pipe(Effect.exit);
					yield* rebuild.Rebuild();

					return {
						acknowledged,
						after_activity,
						before,
						commands: yield* database.client.select().from(JournalCommands),
						duplicate,
						events: yield* database.client.select().from(JournalEvents),
						first,
						future,
						rebuilt: (yield* threads.Snapshot()).threads[0]!,
						stale_acknowledged,
					};
				}),
			);

			expect(result.acknowledged).toMatchObject({
				reader_acknowledged_activity_at: result.acknowledged.reader_activity_at,
			});
			expect(result.acknowledged.metadata_version).toBe(result.before.metadata_version);
			persisted_acknowledged_activity_at = result.rebuilt.reader_acknowledged_activity_at;
			expect(result.duplicate[0]!.payload).toMatchObject({ status: "duplicate" });
			expect(result.first[0]!.payload).toMatchObject({ status: "accepted" });
			expect(
				result.commands.filter(
					(command) => command.payload_type === "thread.attention.acknowledge",
				),
			).toHaveLength(2);
			expect(
				result.events.filter(
					(event) => JSON.parse(event.payload_json).change === "attention_acknowledged",
				),
			).toHaveLength(2);
			expect(result.after_activity.reader_activity_at).not.toBe(
				result.stale_acknowledged.reader_acknowledged_activity_at,
			);
			expect(result.stale_acknowledged.reader_acknowledged_activity_at).toBe(
				result.acknowledged.reader_acknowledged_activity_at,
			);
			expect(result.future).toMatchObject({
				_tag: "Success",
				value: [
					{
						payload: {
							error: { code: "journal.invariant_failed" },
							status: "rejected",
						},
					},
				],
			});
			expect(result.rebuilt.reader_acknowledged_activity_at).toBe(
				result.stale_acknowledged.reader_acknowledged_activity_at,
			);
		} finally {
			await runtime.dispose();
		}

		const restarted = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: MakeRuntimeMetadata(),
		});
		try {
			const thread = await restarted.runPromise(
				Effect.gen(function* () {
					const threads = yield* ThreadReadModel;
					return (yield* threads.Snapshot()).threads[0]!;
				}),
			);
			expect(thread.reader_acknowledged_activity_at).toBe(persisted_acknowledged_activity_at);
		} finally {
			await restarted.dispose();
		}
	});
});
