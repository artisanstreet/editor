import { layer as make_sqlite_layer } from "@effect/sql-sqlite-node/SqliteClient";
import { Context, Effect, Layer } from "effect";
import { migrate } from "drizzle-orm/effect-sqlite-node/migrator";

import * as SQLiteNodeDrizzle from "drizzle-orm/effect-sqlite-node";

export type DatabaseClient = SQLiteNodeDrizzle.EffectSQLiteNodeDatabase;

export interface DatabaseOptions {
	readonly database_path: string;
	readonly migrations_path: string;
}

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

			yield* migrate(client, {
				migrationsFolder: options.migrations_path,
			});

			return { client };
		}),
	);

	return DatabaseLive.pipe(Layer.provide(SqliteLive));
}
