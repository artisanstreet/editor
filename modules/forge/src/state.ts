import { lstat, mkdir, readFile, rm } from "node:fs/promises";
import { dirname } from "node:path";

import { Data, Effect, Schema } from "effect";

import { WriteFileAtomically } from "./atomic-file";

export const ForgeState = Schema.Struct({
	endpoint: Schema.String,
	instance_id: Schema.String,
	pid: Schema.Int,
	started_at: Schema.String,
	version: Schema.Literal(1),
});

export type ForgeState = typeof ForgeState.Type;

export class ForgeStateFailure extends Data.TaggedError("ForgeStateFailure")<{
	readonly cause: unknown;
	readonly operation: "read" | "remove" | "write";
	readonly path: string;
}> {}

const StateFailure = (path: string, operation: ForgeStateFailure["operation"], cause: unknown) =>
	new ForgeStateFailure({ cause, operation, path });

const AssertSafeStateBoundary = (path: string) =>
	Effect.gen(function* () {
		for (const directory of [dirname(path), dirname(dirname(path))]) {
			const metadata = yield* Effect.tryPromise(() => lstat(directory));
			if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
				return yield* Effect.fail(new Error("Unsafe Forge state directory"));
			}
		}
		const metadata = yield* Effect.tryPromise({
			try: () => lstat(path),
			catch: (cause) => cause as NodeJS.ErrnoException,
		}).pipe(
			Effect.map((value) => ({ _tag: "Some" as const, value })),
			Effect.catch((cause: NodeJS.ErrnoException) =>
				cause.code === "ENOENT"
					? Effect.succeed({ _tag: "None" as const })
					: Effect.fail(cause),
			),
		);
		if (
			metadata._tag === "Some" &&
			(!metadata.value.isFile() || metadata.value.isSymbolicLink())
		) {
			return yield* Effect.fail(new Error("Unsafe Forge state file"));
		}
	}).pipe(Effect.mapError((cause) => StateFailure(path, "read", cause)));

/** Writes state through a same-directory replacement so readers never see partial JSON. */
export const WriteForgeState = (path: string, state: ForgeState) =>
	Effect.gen(function* () {
		yield* Effect.tryPromise({
			try: () => mkdir(dirname(path), { recursive: true }),
			catch: (cause) => StateFailure(path, "write", cause),
		});
		yield* AssertSafeStateBoundary(path).pipe(
			Effect.mapError((error) => StateFailure(path, "write", error.cause)),
		);
		yield* WriteFileAtomically(path, `${JSON.stringify(state)}\n`).pipe(
			Effect.mapError((cause) => StateFailure(path, "write", cause)),
		);
	});

/** Removes state only when it still represents this exact daemon instance. */
export const RemoveForgeState = (path: string, instance_id: string) =>
	Effect.gen(function* () {
		yield* AssertSafeStateBoundary(path);
		const encoded = yield* Effect.tryPromise({
			try: () => readFile(path, "utf8"),
			catch: (cause) => StateFailure(path, "read", cause),
		});
		const existing = yield* Schema.decodeUnknownEffect(Schema.fromJsonString(ForgeState))(
			encoded,
		).pipe(Effect.mapError((cause) => StateFailure(path, "read", cause)));
		if (existing.instance_id !== instance_id) return;
		yield* Effect.tryPromise({
			try: () => rm(path, { force: true }),
			catch: (cause) => StateFailure(path, "remove", cause),
		});
	}).pipe(Effect.catch(() => Effect.void));
