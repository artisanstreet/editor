import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	Database,
	DatabaseDiskIoPolicy,
	make_database_layer,
} from "../../modules/backend/src/persistence/database";
const migrations_path = join(import.meta.dirname, "../../modules/backend/drizzle");

describe("database disk I/O policy", () => {
	let directory: string | undefined;

	afterEach(async () => {
		if (directory !== undefined) await rm(directory, { force: true, recursive: true });
	});

	it("configures bounded WAL and memory-backed temporary storage", async () => {
		directory = await mkdtemp(join(tmpdir(), "artisan-database-disk-io-policy-"));
		const database_path = join(directory, "artisan.sqlite");

		const policy = await Effect.runPromise(
			Effect.gen(function* () {
				const database = yield* Database;
				const [cache_size] = yield* database.client.all<{ cache_size: number }>(
					"PRAGMA cache_size",
				);
				const [auto_vacuum] = yield* database.client.all<{ auto_vacuum: number }>(
					"PRAGMA auto_vacuum",
				);
				const [journal_size_limit] = yield* database.client.all<{
					journal_size_limit: number;
				}>("PRAGMA journal_size_limit");
				const [synchronous] = yield* database.client.all<{ synchronous: number }>(
					"PRAGMA synchronous",
				);
				const [temp_store] = yield* database.client.all<{ temp_store: number }>(
					"PRAGMA temp_store",
				);
				const [wal_autocheckpoint] = yield* database.client.all<{
					wal_autocheckpoint: number;
				}>("PRAGMA wal_autocheckpoint");

				return {
					auto_vacuum: auto_vacuum?.auto_vacuum,
					cache_size: cache_size?.cache_size,
					journal_size_limit: journal_size_limit?.journal_size_limit,
					synchronous: synchronous?.synchronous,
					temp_store: temp_store?.temp_store,
					wal_autocheckpoint: wal_autocheckpoint?.wal_autocheckpoint,
				};
			}).pipe(
				Effect.provide(
					make_database_layer({
						database_path,
						migrations_path,
					}),
				),
				Effect.scoped,
			),
		);

		expect(policy).toEqual({
			auto_vacuum: 2,
			cache_size: -DatabaseDiskIoPolicy.cache_size_kib,
			journal_size_limit: DatabaseDiskIoPolicy.journal_size_limit_bytes,
			synchronous: 1,
			temp_store: 2,
			wal_autocheckpoint: DatabaseDiskIoPolicy.wal_autocheckpoint_pages,
		});
	});
});
