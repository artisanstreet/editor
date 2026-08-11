import { layer as make_sqlite_layer } from "@effect/sql-sqlite-node/SqliteClient";
import { Context, Effect, Layer } from "effect";
import { migrate } from "drizzle-orm/effect-sqlite-node/migrator";

import * as SQLiteNodeDrizzle from "drizzle-orm/effect-sqlite-node";

export type DatabaseClient = SQLiteNodeDrizzle.EffectSQLiteNodeDatabase;

export interface DatabaseOptions {
	readonly database_path: string;
	readonly migrations_path: string;
}

/**
 * Reduces SQLite's transient disk churn while retaining ordinary WAL crash
 * resilience. `NORMAL` may lose the most recent commits after OS or power
 * failure, and checkpoint thresholds are not hard WAL-size ceilings while a
 * reader holds a snapshot. Packaged qualification still measures actual I/O.
 */
export const DatabaseDiskIoPolicy = {
	cache_size_kib: 64 * 1_024,
	journal_size_limit_bytes: 8 * 1_024 * 1_024,
	wal_autocheckpoint_pages: 1_000,
} as const;

export class Database extends Context.Service<
	Database,
	{
		readonly client: DatabaseClient;
	}
>()("Artisan/Database") {}

export function make_database_layer(options: DatabaseOptions) {
	const SqliteLive = make_sqlite_layer({
		disableWAL: false,
		filename: options.database_path,
	});

	const DatabaseLive = Layer.effect(
		Database,
		Effect.gen(function* () {
			const client = yield* SQLiteNodeDrizzle.makeWithDefaults();

			const [schema_count] = yield* client.all<{ count: number }>(
				"SELECT COUNT(*) AS count FROM sqlite_schema",
			);
			if ((schema_count?.count ?? 0) === 0) {
				/** Enables bounded page reclamation before a fresh store has data to rewrite. */
				yield* client.run("PRAGMA auto_vacuum = INCREMENTAL");
				yield* client.run("VACUUM");
			}
			yield* migrate(client, {
				migrationsFolder: options.migrations_path,
			});

			yield* client.run("PRAGMA synchronous = NORMAL");
			yield* client.run("PRAGMA temp_store = MEMORY");
			yield* client.run(`PRAGMA cache_size = -${DatabaseDiskIoPolicy.cache_size_kib}`);
			yield* client.run(
				`PRAGMA journal_size_limit = ${DatabaseDiskIoPolicy.journal_size_limit_bytes}`,
			);
			yield* client.run(
				`PRAGMA wal_autocheckpoint = ${DatabaseDiskIoPolicy.wal_autocheckpoint_pages}`,
			);
			/** Reclaims a bounded number of already-free tail pages without a full VACUUM. */
			yield* client.run("PRAGMA incremental_vacuum(1024)");

			return { client };
		}),
	);

	return DatabaseLive.pipe(Layer.provide(SqliteLive));
}
