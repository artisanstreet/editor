import { mkdir, open, readFile, unlink } from "node:fs/promises";
import { dirname } from "node:path";

import { MakeSnowflakeIdLive, SnowflakeId } from "@artisan/protocol";
import { Data, Effect, Layer, Schema } from "effect";

export class ForgeDatabaseAlreadyOwned extends Data.TaggedError("ForgeDatabaseAlreadyOwned")<{
	readonly database_path: string;
	readonly lock_path: string;
}> {}

export class ForgeDatabaseLeaseFailure extends Data.TaggedError("ForgeDatabaseLeaseFailure")<{
	readonly cause: unknown;
	readonly lock_path: string;
}> {}

export interface ForgeDatabaseLease {
	readonly lock_path: string;
	readonly Release: Effect.Effect<void, ForgeDatabaseLeaseFailure>;
}

const DatabaseLeaseOwner = Schema.Struct({
	instance_id: Schema.String,
	pid: Schema.Int,
});

type DatabaseLeaseOwner = typeof DatabaseLeaseOwner.Type;

const IsProcessAlive = (pid: number) => {
	try {
		process.kill(pid, 0);
		return true;
	} catch (cause) {
		return (cause as NodeJS.ErrnoException).code === "EPERM";
	}
};

const ReadLeaseOwner = (lock_path: string) =>
	Effect.gen(function* () {
		const encoded = yield* Effect.tryPromise(() => readFile(lock_path, "utf8"));
		const parsed = yield* Effect.try(() => JSON.parse(encoded));
		return yield* Schema.decodeUnknownEffect(DatabaseLeaseOwner)(parsed);
	}).pipe(Effect.option);

const RemoveLease = (lock_path: string) =>
	Effect.tryPromise({
		try: () => unlink(lock_path),
		catch: (cause) => cause as NodeJS.ErrnoException,
	}).pipe(
		Effect.catch((cause: NodeJS.ErrnoException) =>
			cause.code === "ENOENT" ? Effect.void : Effect.fail(cause),
		),
	);

const WriteLeaseOwner = (lock_path: string, owner: DatabaseLeaseOwner) =>
	Effect.acquireUseRelease(
		Effect.tryPromise({
			try: () => open(lock_path, "wx"),
			catch: (cause) => cause as NodeJS.ErrnoException,
		}),
		(handle) =>
			Effect.tryPromise({
				try: () => handle.writeFile(`${JSON.stringify(owner)}\n`, "utf8"),
				catch: (cause) => cause as NodeJS.ErrnoException,
			}),
		(handle) => Effect.promise(() => handle.close()).pipe(Effect.ignore),
	);

/**
 * Uses an adjacent exclusive-create lease so two independent host processes
 * cannot both perform migrations or claim engine/workspace ownership.
 */
export function acquire_forge_database_lease(
	database_path: string,
): Effect.Effect<ForgeDatabaseLease, ForgeDatabaseAlreadyOwned | ForgeDatabaseLeaseFailure> {
	const lock_path = `${database_path}.artisan-forge.lock`;

	return Effect.gen(function* () {
		const owner: DatabaseLeaseOwner = {
			instance_id: yield* (yield* SnowflakeId).Make("lease"),
			pid: process.pid,
		};
		yield* Effect.tryPromise({
			try: () => mkdir(dirname(database_path), { recursive: true }),
			catch: (cause) => cause,
		});

		const Acquire = (may_recover_stale: boolean): Effect.Effect<void, unknown> =>
			WriteLeaseOwner(lock_path, owner).pipe(
				Effect.catch((cause: NodeJS.ErrnoException) => {
					if (cause.code !== "EEXIST") return Effect.fail(cause);
					return Effect.gen(function* () {
						const current_owner = yield* ReadLeaseOwner(lock_path);
						if (
							!may_recover_stale ||
							current_owner._tag === "None" ||
							IsProcessAlive(current_owner.value.pid)
						) {
							return yield* Effect.fail(
								new ForgeDatabaseAlreadyOwned({ database_path, lock_path }),
							);
						}
						yield* RemoveLease(lock_path);
						yield* Acquire(false);
					});
				}),
			);

		yield* Acquire(true);
		return {
			lock_path,
			Release: Effect.gen(function* () {
				const current_owner = yield* ReadLeaseOwner(lock_path);
				if (
					current_owner._tag === "None" ||
					current_owner.value.instance_id !== owner.instance_id
				) {
					return;
				}
				yield* RemoveLease(lock_path);
			}).pipe(
				Effect.mapError((cause) => new ForgeDatabaseLeaseFailure({ cause, lock_path })),
			),
		};
	}).pipe(
		Effect.mapError((cause) =>
			cause instanceof ForgeDatabaseAlreadyOwned
				? cause
				: new ForgeDatabaseLeaseFailure({ cause, lock_path }),
		),
		Effect.provide(MakeSnowflakeIdLive(2).pipe(Layer.orDie)),
	);
}

/** Acquires a database lease and releases it automatically with the current Scope. */
export const AcquireForgeDatabaseLease = (database_path: string) =>
	Effect.acquireRelease(acquire_forge_database_lease(database_path), (lease) =>
		lease.Release.pipe(Effect.ignore),
	);
