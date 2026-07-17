import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import {
	ExportControlAuditDecisions,
	SurfaceProjectionGenerations,
	SurfaceProjectionItems,
	SurfaceProjectionState,
	Threads,
} from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const surface_export_migration = "20260717115855_supreme_ghost_rider";
const temporary_directories: Array<string> = [];
const timestamp = "2026-07-17T12:00:00.000Z";

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-surface-export-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);
	const prior_entries = entries.filter((entry) => entry < surface_export_migration);

	temporary_directories.push(directory);
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		prior_entries,
		(entry) =>
			file_system.copy(join(migrations_path, entry), join(prior_migrations_path, entry)),
		{ concurrency: "unbounded" },
	);

	return { database_path, prior_migrations_path };
}).pipe(Effect.provide(NodeFileSystem.layer));

afterEach(async () => {
	const directories = temporary_directories.splice(0);

	await Effect.runPromise(
		Effect.forEach(
			directories,
			(directory) =>
				FileSystem.FileSystem.pipe(
					Effect.flatMap((file_system) =>
						file_system.remove(directory, { recursive: true }),
					),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("surface and export-control migration", () => {
	it("preserves existing data while creating empty durable stores", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.prior_migrations_path,
			}),
		);

		try {
			await prior_runtime.runPromise(
				Database.pipe(
					Effect.flatMap((database) =>
						database.client.run(`
							INSERT INTO threads (thread_id, title, created_at, updated_at)
							VALUES ('thread_surface_migration', 'Surface migration', '${timestamp}', '${timestamp}')
						`),
					),
				),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const current_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path,
			}),
		);

		try {
			const result = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;
					const threads = yield* database.client.select().from(Threads);
					const generations = yield* database.client
						.select()
						.from(SurfaceProjectionGenerations);
					const items = yield* database.client.select().from(SurfaceProjectionItems);
					const state = yield* database.client.select().from(SurfaceProjectionState);
					const audits = yield* database.client
						.select()
						.from(ExportControlAuditDecisions);

					return { audits, generations, items, state, threads };
				}),
			);

			expect(result.threads).toEqual([
				expect.objectContaining({
					thread_id: "thread_surface_migration",
					title: "Surface migration",
				}),
			]);
			expect(result.generations).toEqual([]);
			expect(result.items).toEqual([]);
			expect(result.state).toEqual([]);
			expect(result.audits).toEqual([]);
		} finally {
			await current_runtime.dispose();
		}
	});
});
