import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";

import { make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	JournalStore,
	JournalStoreLive,
} from "../../modules/backend/src/persistence/journal-store";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";
import {
	SurfaceProjectionRebuilder,
	SurfaceProjectionRebuilderLive,
} from "../../modules/backend/src/surface/surface-projection-rebuilder";
import {
	SurfaceProjectionStore,
	type SurfaceProjectionSnapshot,
} from "../../modules/backend/src/surface/surface-projection-store";
import { SurfaceProjectorLive } from "../../modules/backend/src/surface/surface-projector";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-surface-rebuild-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_store() {
	let current: SurfaceProjectionSnapshot = {
		items: [],
		stream_cursors: [],
		watermark: 0,
	};
	const layer = Layer.succeed(SurfaceProjectionStore, {
		Read: Effect.sync(() => current),
		Replace: (snapshot) =>
			Effect.sync(() => {
				current = snapshot;

				return current;
			}),
	});

	return {
		layer,
		read: () => current,
		replace_for_test: (snapshot: SurfaceProjectionSnapshot) => {
			current = snapshot;
		},
	};
}

function make_runtime(database_path: string, store: ReturnType<typeof make_store>) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		JournalNotifierLive,
		RuntimeMetadataLive,
	);
	const journal = JournalStoreLive.pipe(Layer.provide(infrastructure));
	const dependencies = Layer.mergeAll(journal, SurfaceProjectorLive, store.layer);
	const rebuilder = SurfaceProjectionRebuilderLive.pipe(Layer.provide(dependencies));

	return ManagedRuntime.make(Layer.mergeAll(infrastructure, dependencies, rebuilder));
}

function create_command(): CommandEnvelope {
	return {
		kind: "command",
		message_id: "command_surface_thread",
		origin: "frontend",
		payload: { title: "Surface rebuild", type: "thread.create" },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-17T12:00:00.000Z",
		thread_id: "thread_surface",
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

describe("SurfaceProjectionRebuilder", () => {
	it("diagnoses drift and deterministically restores a deleted generation", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const store = make_store();
		const runtime = make_runtime(database_path, store);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const rebuilder = yield* SurfaceProjectionRebuilder;

					yield* journal.AcceptThreadCreate(create_command());
					yield* journal.AppendEvent({
						causation_id: "run_started",
						correlation_id: "run_started",
						payload: {
							state: "running",
							type: "run.lifecycle",
							working_directory: "C:/artisan",
						},
						run_id: "run_surface",
						thread_id: "thread_surface",
					});
					yield* journal.AppendEvent({
						causation_id: "run_usage",
						correlation_id: "run_usage",
						payload: {
							type: "run.usage.updated",
							usage: { input_tokens: 21, output_tokens: 34 },
						},
						run_id: "run_surface",
						thread_id: "thread_surface",
					});

					const before = yield* rebuilder.Verify;
					const rebuilt = yield* rebuilder.Rebuild;
					const equivalent = yield* rebuilder.Verify;

					store.replace_for_test({ items: [], stream_cursors: [], watermark: 0 });

					const deleted = yield* rebuilder.Verify;
					const restored = yield* rebuilder.Rebuild;
					const restored_equivalent = yield* rebuilder.Verify;

					return {
						before,
						deleted,
						equivalent,
						rebuilt,
						restored,
						restored_equivalent,
					};
				}),
			);

			expect(result.before).toMatchObject({
				equivalent: false,
				missing_surface_ids: ["surface:run:run_surface", "surface:thread:thread_surface"],
			});
			expect(result.equivalent).toMatchObject({ equivalent: true });
			expect(result.deleted).toEqual(result.before);
			expect(result.restored).toEqual(result.rebuilt);
			expect(result.restored_equivalent).toMatchObject({ equivalent: true });
			expect(store.read().items).toEqual(
				expect.arrayContaining([
					expect.objectContaining({
						state: "running",
						surface_id: "surface:run:run_surface",
						usage: { input_tokens: 21, output_tokens: 34 },
					}),
				]),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("reports changed and unexpected items without replacing them during verification", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const store = make_store();
		const runtime = make_runtime(database_path, store);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const rebuilder = yield* SurfaceProjectionRebuilder;

					yield* journal.AcceptThreadCreate(create_command());

					const rebuilt = yield* rebuilder.Rebuild;
					const [thread] = rebuilt.items;

					if (!thread) {
						return yield* Effect.die("Expected a rebuilt thread surface");
					}

					store.replace_for_test({
						...rebuilt,
						items: [
							{ ...thread, state: "corrupt" },
							{ ...thread, surface_id: "surface:unexpected" },
						],
					});

					return yield* rebuilder.Verify;
				}),
			);

			expect(result).toMatchObject({
				changed_surface_ids: ["surface:thread:thread_surface"],
				equivalent: false,
				missing_surface_ids: [],
				unexpected_surface_ids: ["surface:unexpected"],
			});
			expect(store.read().items).toHaveLength(2);
		} finally {
			await runtime.dispose();
		}
	});

	it("reports stale, missing, and unexpected stream cursors", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const store = make_store();
		const runtime = make_runtime(database_path, store);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const journal = yield* JournalStore;
					const rebuilder = yield* SurfaceProjectionRebuilder;

					yield* journal.AcceptThreadCreate(create_command());
					yield* journal.AcceptThreadCreate({
						...create_command(),
						message_id: "command_surface_cursor_other",
						payload: { title: "Other surface", type: "thread.create" },
						thread_id: "thread_surface_cursor_other",
					});
					yield* journal.AppendEvent({
						causation_id: "run_started_cursor",
						correlation_id: "run_started_cursor",
						payload: {
							state: "running",
							type: "run.lifecycle",
							working_directory: "C:/artisan",
						},
						run_id: "run_cursor",
						thread_id: "thread_surface",
					});
					yield* journal.AppendEvent({
						causation_id: "run_completed_cursor",
						correlation_id: "run_completed_cursor",
						payload: {
							state: "completed",
							type: "run.lifecycle",
							working_directory: "C:/artisan",
						},
						run_id: "run_cursor",
						thread_id: "thread_surface",
					});

					const rebuilt = yield* rebuilder.Rebuild;
					const changed = rebuilt.stream_cursors.find(({ sequence }) => sequence >= 2);
					const missing = rebuilt.stream_cursors.find(
						({ stream_id }) => stream_id !== changed?.stream_id,
					);

					if (!missing || !changed) {
						return yield* Effect.die("Expected two projection cursor streams");
					}

					store.replace_for_test({
						...rebuilt,
						stream_cursors: [
							{ ...changed, sequence: changed.sequence - 1 },
							{ sequence: 1, stream_id: "unexpected:stream" },
						],
					});

					const difference = yield* rebuilder.Verify;

					return { changed: changed.stream_id, difference, missing: missing.stream_id };
				}),
			);

			expect(result.difference).toMatchObject({
				changed_stream_ids: [result.changed],
				equivalent: false,
				missing_stream_ids: [result.missing],
				unexpected_stream_ids: ["unexpected:stream"],
			});
		} finally {
			await runtime.dispose();
		}
	});
});
