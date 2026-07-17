import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";

import { make_database_layer, Database } from "../../modules/backend/src/persistence/database";
import {
	JournalInvariantError,
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-journal-snapshot-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		JournalNotifierLive,
		RuntimeMetadataLive,
	);
	const journal = JournalStoreLive.pipe(Layer.provide(infrastructure));

	return ManagedRuntime.make(Layer.mergeAll(infrastructure, journal));
}

function create_command(): CommandEnvelope {
	return {
		kind: "command",
		message_id: "command_thread_create",
		origin: "frontend",
		payload: { title: "Projection rebuild", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-17T12:00:00.000Z",
		thread_id: "thread_projection",
	};
}

afterEach(async () => {
	const cleanup = directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(cleanup, (directory) =>
			FileSystem.FileSystem.pipe(
				Effect.flatMap((file_system) => file_system.remove(directory, { recursive: true })),
			),
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("JournalStore.ReadSnapshot", () => {
	it("returns one fixed strict watermark with matching stream cursors", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;

					yield* journal.AcceptThreadCreate(create_command());
					yield* journal.AppendEvent({
						causation_id: "message_completed",
						correlation_id: "message_completed",
						payload: {
							message_id: "assistant_message_1",
							text: "The projection boundary is ready.",
							type: "assistant.message_completed",
						},
						thread_id: "thread_projection",
					});

					const first = yield* journal.ReadSnapshot();

					yield* journal.AppendEvent({
						causation_id: "run_completed",
						correlation_id: "run_completed",
						payload: {
							state: "completed",
							type: "run.lifecycle",
							working_directory: "C:/artisan",
						},
						run_id: "run_projection",
						thread_id: "thread_projection",
					});

					const second = yield* journal.ReadSnapshot();

					return { first, second };
				}),
			);

			expect(result.first.events).toHaveLength(2);
			expect(result.first.watermark).toBe(result.first.events.at(-1)?.journal_sequence);
			expect(result.first.stream_cursors).toEqual([
				{ sequence: 2, stream_id: "thread:thread_projection" },
			]);
			expect(result.second.events).toHaveLength(3);
			expect(result.second.watermark).toBe(result.second.events.at(-1)?.journal_sequence);
			expect(result.second.stream_cursors).toEqual([
				{ sequence: 3, stream_id: "thread:thread_projection" },
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when persisted stream cursors disagree with the ledger", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const journal = yield* JournalStore;

					yield* journal.AcceptThreadCreate(create_command());
					yield* database.client.run(
						"UPDATE event_streams SET last_sequence = 2 WHERE stream_id = 'thread:thread_projection'",
					);

					return yield* Effect.flip(journal.ReadSnapshot());
				}),
			);

			expect(failure).toBeInstanceOf(JournalInvariantError);
		} finally {
			await runtime.dispose();
		}
	});
});
