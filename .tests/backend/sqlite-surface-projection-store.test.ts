import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, Layer, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { SurfaceItem } from "@artisan/protocol";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	SurfaceProjectionGenerations,
	SurfaceProjectionItems,
} from "../../modules/backend/src/persistence/schema";
import { RuntimeMetadataLive } from "../../modules/backend/src/runtime/runtime-metadata";
import { SQLiteSurfaceProjectionStoreLive } from "../../modules/backend/src/surface/sqlite-surface-projection-store";
import {
	SurfaceProjectionStore,
	SurfaceProjectionStoreConflict,
	SurfaceProjectionStoreFailure,
	type SurfaceProjectionSnapshot,
} from "../../modules/backend/src/surface/surface-projection-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];

const MakeDatabasePath = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-surface-projection-store-",
	});

	directories.push(directory);

	return `${directory}/artisan.db`;
}).pipe(Effect.provide(NodeFileSystem.layer));

function make_runtime(database_path: string) {
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path, migrations_path }),
		RuntimeMetadataLive,
	);
	const store = SQLiteSurfaceProjectionStoreLive.pipe(Layer.provide(infrastructure));

	return ManagedRuntime.make(Layer.mergeAll(infrastructure, store));
}

function surface_item(surface_id: string, state: string): SurfaceItem {
	return {
		group: "Work",
		kind: "thread",
		label: `Thread ${surface_id}`,
		source: "artisan",
		state,
		summary: `State ${state}`,
		surface_id,
		thread_id: surface_id,
		timestamp: "2026-07-17T12:00:00.000Z",
	};
}

function snapshot(watermark: number, items: ReadonlyArray<SurfaceItem>): SurfaceProjectionSnapshot {
	return {
		items,
		stream_cursors:
			watermark === 0 ? [] : [{ sequence: watermark, stream_id: "thread:surface" }],
		watermark,
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

describe("SQLiteSurfaceProjectionStore", () => {
	it("atomically replaces immutable generations and removes superseded rows", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const first = snapshot(1, [surface_item("surface:first", "running")]);
		const second = snapshot(2, [
			surface_item("surface:first", "completed"),
			surface_item("surface:second", "running"),
		]);

		try {
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* SurfaceProjectionStore;
					const empty = yield* store.Read;

					yield* store.Replace(first);
					const persisted_first = yield* store.Read;

					yield* store.Replace(second);
					const persisted_second = yield* store.Read;
					const generations = yield* database.client
						.select({ generation_id: SurfaceProjectionGenerations.generation_id })
						.from(SurfaceProjectionGenerations);
					const items = yield* database.client
						.select({ surface_id: SurfaceProjectionItems.surface_id })
						.from(SurfaceProjectionItems);

					return { empty, generations, items, persisted_first, persisted_second };
				}),
			);

			expect(result.empty).toEqual(snapshot(0, []));
			expect(result.persisted_first).toEqual(first);
			expect(result.persisted_second).toEqual(second);
			expect(result.generations).toHaveLength(1);
			expect(result.items.map(({ surface_id }) => surface_id).sort()).toEqual([
				"surface:first",
				"surface:second",
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("never exposes a partial generation to concurrent readers", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const writer = make_runtime(database_path);
		const reader = make_runtime(database_path);
		const first = snapshot(1, [surface_item("surface:first", "running")]);
		const second = snapshot(2, [
			surface_item("surface:first", "completed"),
			surface_item("surface:second", "running"),
		]);

		try {
			await writer.runPromise(
				SurfaceProjectionStore.pipe(Effect.flatMap((store) => store.Replace(first))),
			);

			const reads = Array.from({ length: 32 }, () =>
				reader.runPromise(
					SurfaceProjectionStore.pipe(Effect.flatMap((store) => store.Read)),
				),
			);
			const [, ...observed] = await Promise.all([
				writer.runPromise(
					SurfaceProjectionStore.pipe(Effect.flatMap((store) => store.Replace(second))),
				),
				...reads,
			]);

			for (const value of observed) {
				expect([first, second]).toContainEqual(value);
			}
		} finally {
			await Promise.all([writer.dispose(), reader.dispose()]);
		}
	});

	it("rejects stale two-runtime replacement without moving the active watermark", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const first_runtime = make_runtime(database_path);
		const second_runtime = make_runtime(database_path);
		const current = snapshot(2, [surface_item("surface:current", "running")]);
		const stale = snapshot(1, [surface_item("surface:stale", "running")]);

		try {
			await first_runtime.runPromise(
				SurfaceProjectionStore.pipe(Effect.flatMap((store) => store.Replace(current))),
			);
			const failure = await second_runtime.runPromise(
				SurfaceProjectionStore.pipe(
					Effect.flatMap((store) => Effect.flip(store.Replace(stale))),
				),
			);
			const persisted = await second_runtime.runPromise(
				SurfaceProjectionStore.pipe(Effect.flatMap((store) => store.Read)),
			);

			expect(failure).toBeInstanceOf(SurfaceProjectionStoreConflict);
			expect(persisted).toEqual(current);
		} finally {
			await Promise.all([first_runtime.dispose(), second_runtime.dispose()]);
		}
	});

	it("rejects a stream cursor beyond the fixed journal watermark", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const invalid: SurfaceProjectionSnapshot = {
			items: [surface_item("surface:future", "running")],
			stream_cursors: [{ sequence: 2, stream_id: "thread:surface" }],
			watermark: 1,
		};

		try {
			const failure = await runtime.runPromise(
				SurfaceProjectionStore.pipe(
					Effect.flatMap((store) => Effect.flip(store.Replace(invalid))),
				),
			);

			expect(failure).toBeInstanceOf(SurfaceProjectionStoreConflict);
		} finally {
			await runtime.dispose();
		}
	});

	it("fails closed when a stored item no longer decodes as a canonical surface", async () => {
		const database_path = await Effect.runPromise(MakeDatabasePath);
		const runtime = make_runtime(database_path);
		const current = snapshot(1, [surface_item("surface:corrupt", "running")]);

		try {
			const failure = await runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const store = yield* SurfaceProjectionStore;

					yield* store.Replace(current);
					yield* database.client.update(SurfaceProjectionItems).set({ item_json: "{}" });

					return yield* Effect.flip(store.Read);
				}),
			);

			expect(failure).toBeInstanceOf(SurfaceProjectionStoreFailure);
		} finally {
			await runtime.dispose();
		}
	});
});
