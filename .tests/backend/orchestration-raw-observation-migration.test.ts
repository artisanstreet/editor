import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, FileSystem, ManagedRuntime } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Database, make_database_layer } from "../../modules/backend/src/persistence/database";
import { OrchestrationRawObservations } from "../../modules/backend/src/persistence/schema";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const target_migration = "20260716103602_third_nocturne";
const temporary_directories: Array<string> = [];

const MakeMigrationPaths = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const directory = yield* file_system.makeTempDirectory({
		prefix: "artisan-raw-observation-migration-",
	});
	const prior_migrations_path = join(directory, "prior-drizzle");
	const database_path = join(directory, "artisan.db");
	const entries = yield* file_system.readDirectory(migrations_path);

	yield* Effect.sync(() => temporary_directories.push(directory));
	yield* file_system.makeDirectory(prior_migrations_path, { recursive: true });
	yield* Effect.forEach(
		entries.filter((entry) => entry < target_migration),
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
				Effect.flatMap(FileSystem.FileSystem, (file_system) =>
					file_system.remove(directory, { recursive: true }),
				),
			{ discard: true },
		).pipe(Effect.provide(NodeFileSystem.layer)),
	);
});

describe("orchestration raw-observation migration", () => {
	it("preserves legacy rows and scopes observation identity to each run", async () => {
		const paths = await Effect.runPromise(MakeMigrationPaths);
		const prior_runtime = ManagedRuntime.make(
			make_database_layer({
				database_path: paths.database_path,
				migrations_path: paths.prior_migrations_path,
			}),
		);

		try {
			await prior_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(OrchestrationRawObservations).values({
						engine_id: "engine_legacy",
						frame_json: '{"legacy":true}',
						observation_id: "shared_observation",
						run_id: "run_legacy",
						sequence: 1,
						transport: "stdio-jsonl",
					});
				}),
			);
		} finally {
			await prior_runtime.dispose();
		}

		const current_runtime = ManagedRuntime.make(
			make_database_layer({ database_path: paths.database_path, migrations_path }),
		);

		try {
			const observations = await current_runtime.runPromise(
				Effect.gen(function* () {
					const database = yield* Database;

					yield* database.client.insert(OrchestrationRawObservations).values({
						engine_id: "engine_current",
						frame_json: '{"current":true}',
						observation_id: "shared_observation",
						run_id: "run_current",
						sequence: 2,
						transport: "stdio-jsonl",
					});

					return yield* database.client.select().from(OrchestrationRawObservations);
				}),
			);

			expect(observations).toEqual([
				expect.objectContaining({
					frame_json: '{"legacy":true}',
					observation_id: "shared_observation",
					run_id: "run_legacy",
				}),
				expect.objectContaining({
					frame_json: '{"current":true}',
					observation_id: "shared_observation",
					run_id: "run_current",
				}),
			]);
		} finally {
			await current_runtime.dispose();
		}
	});
});
