import { constants } from "node:fs";
import { access, link, open, readFile, rename, rm, stat, statfs, unlink } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";

import { Console, Effect, Schema } from "effect";

const Arguments = Schema.Struct({ database_path: Schema.NonEmptyString });

const ReadArguments = Effect.gen(function* () {
	const explicit_index = process.argv.indexOf("--database");
	const explicit_path = explicit_index < 0 ? undefined : process.argv[explicit_index + 1];
	const local_app_data = process.env.LOCALAPPDATA;
	return yield* Schema.decodeUnknownEffect(Arguments)({
		database_path:
			explicit_path ??
			(local_app_data === undefined
				? ""
				: join(local_app_data, "Artisan", "data", "artisan.sqlite")),
	});
});

const Exists = (path: string) =>
	Effect.tryPromise(() => access(path, constants.F_OK)).pipe(
		Effect.as(true),
		Effect.catch(() => Effect.succeed(false)),
	);

const IsProcessAlive = (pid: number) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch (cause) {
		return (cause as NodeJS.ErrnoException).code === "EPERM";
	}
};

/** Native Node cannot load the workspace's extensionless TS exports, so this uses Forge's lock format directly. */
const AcquireDatabaseLease = (database_path: string) => {
	const lock_path = `${database_path}.artisan-forge.lock`;
	const instance_id = `compact-${process.pid}-${Date.now()}`;
	const Acquire = (may_recover_stale: boolean): Effect.Effect<void, Error> =>
		Effect.tryPromise({
			try: async () => {
				const handle = await open(lock_path, "wx");
				try {
					await handle.writeFile(
						`${JSON.stringify({ instance_id, pid: process.pid })}\n`,
					);
				} finally {
					await handle.close();
				}
			},
			catch: (cause) => cause as Error,
		}).pipe(
			Effect.catch((cause) =>
				Effect.gen(function* () {
					if ((cause as NodeJS.ErrnoException).code !== "EEXIST")
						return yield* Effect.fail(cause);
					const owner = yield* Effect.tryPromise({
						try: async () =>
							JSON.parse(await readFile(lock_path, "utf8")) as { pid?: number },
						catch: () => ({}),
					});
					if (
						!may_recover_stale ||
						owner.pid === undefined ||
						IsProcessAlive(owner.pid)
					) {
						return yield* Effect.fail(
							new Error(`Forge owns database lease: ${lock_path}`),
						);
					}
					yield* Effect.tryPromise(() => unlink(lock_path));
					yield* Acquire(false);
				}),
			),
		);
	return Effect.acquireRelease(Acquire(true), () =>
		Effect.tryPromise(async () => {
			const owner = JSON.parse(await readFile(lock_path, "utf8")) as { instance_id?: string };
			if (owner.instance_id === instance_id) await unlink(lock_path);
		}).pipe(Effect.ignore),
	);
};

const Compact = (database_path: string) =>
	Effect.gen(function* () {
		const resolved_path = resolve(database_path);
		if (!(yield* Exists(resolved_path))) {
			return yield* Effect.fail(new Error(`Database does not exist: ${resolved_path}`));
		}
		yield* AcquireDatabaseLease(resolved_path);
		if ((yield* Exists(`${resolved_path}-shm`)) || (yield* Exists(`${resolved_path}-wal`))) {
			return yield* Effect.fail(
				new Error(
					"Forge is active or did not close cleanly; run `ae stop` before compaction",
				),
			);
		}

		const source_stat = yield* Effect.tryPromise(() => stat(resolved_path));
		const filesystem = yield* Effect.tryPromise(() => statfs(dirname(resolved_path)));
		const available_bytes = filesystem.bavail * filesystem.bsize;
		if (available_bytes < source_stat.size + 64 * 1_024 * 1_024) {
			return yield* Effect.fail(
				new Error(
					`Compaction needs at least ${source_stat.size + 64 * 1_024 * 1_024} free bytes`,
				),
			);
		}

		const temporary_path = `${resolved_path}.compact`;
		const backup_path = `${resolved_path}.precompact`;
		yield* Effect.tryPromise(() => rm(temporary_path, { force: true }));
		yield* Effect.tryPromise(() => rm(backup_path, { force: true }));

		yield* Effect.acquireUseRelease(
			Effect.sync(() => new DatabaseSync(resolved_path)),
			(database) =>
				Effect.sync(() => {
					const integrity = database.prepare("PRAGMA quick_check").get() as
						| Record<string, unknown>
						| undefined;
					if (Object.values(integrity ?? {})[0] !== "ok") {
						throw new Error("Source database failed SQLite quick_check");
					}
					const remaining_raw = database
						.prepare("SELECT COUNT(*) AS count FROM orchestration_raw_observations")
						.get() as { count: number };
					if (remaining_raw.count !== 0) {
						throw new Error(
							"Storage migration has not run; start the updated Forge once, stop it, then retry",
						);
					}
					database.exec("PRAGMA auto_vacuum = INCREMENTAL");
					database.exec(`VACUUM INTO '${temporary_path.replaceAll("'", "''")}'`);
				}),
			(database) => Effect.sync(() => database.close()),
		);

		yield* Effect.acquireUseRelease(
			Effect.sync(() => new DatabaseSync(temporary_path, { readOnly: true })),
			(database) =>
				Effect.sync(() => {
					const integrity = database.prepare("PRAGMA integrity_check").get() as
						| Record<string, unknown>
						| undefined;
					if (Object.values(integrity ?? {})[0] !== "ok") {
						throw new Error("Compacted database failed SQLite integrity_check");
					}
					const auto_vacuum = database.prepare("PRAGMA auto_vacuum").get() as
						| Record<string, unknown>
						| undefined;
					if (Object.values(auto_vacuum ?? {})[0] !== 2) {
						throw new Error("Compacted database did not enable incremental vacuum");
					}
				}),
			(database) => Effect.sync(() => database.close()),
		);

		if ((yield* Exists(`${resolved_path}-shm`)) || (yield* Exists(`${resolved_path}-wal`))) {
			return yield* Effect.fail(
				new Error("Forge reopened during compaction; no files replaced"),
			);
		}

		/** A hard-link backup plus one atomic replacement never removes the canonical path. */
		yield* Effect.tryPromise(() => link(resolved_path, backup_path));
		yield* Effect.tryPromise(() => rename(temporary_path, resolved_path));
		yield* Effect.tryPromise(() => rm(backup_path, { force: true }));
		const compacted_stat = yield* Effect.tryPromise(() => stat(resolved_path));

		yield* Console.log(
			`Compacted ${resolved_path}: ${source_stat.size} -> ${compacted_stat.size} bytes`,
		);
	});

const Program = Effect.gen(function* () {
	const input = yield* ReadArguments;
	yield* Compact(input.database_path);
}).pipe(
	Effect.catch((cause) => Console.error(String(cause)).pipe(Effect.andThen(Effect.fail(cause)))),
	Effect.scoped,
);

Effect.runPromise(Program).catch(() => {
	process.exitCode = 1;
});
