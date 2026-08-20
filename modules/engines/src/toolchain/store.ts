import { randomUUID } from "node:crypto";

import { Data, Effect, Option, type FileSystem, type Path, Schema } from "effect";

import { ToolchainSemanticVersion } from "./distribution";

const SafeBasename = Schema.String.check(
	Schema.isMinLength(1),
	Schema.isPattern(/^[A-Za-z0-9][A-Za-z0-9._-]*$/),
);

/** One installed binary generation: the version directory plus its file name. */
export const ToolchainGeneration = Schema.Struct({
	binary: SafeBasename,
	/** Immutable unique directory; permits a repaired copy of the same version. */
	directory: SafeBasename,
	sha256: Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/)),
	version: ToolchainSemanticVersion,
});
export type ToolchainGeneration = typeof ToolchainGeneration.Type;

/**
 * The durable per-engine installation record. Absence of the file means the
 * engine has never been managed; `previous` is retained solely for rollback.
 */
export const ToolchainInstallationState = Schema.Struct({
	active: ToolchainGeneration,
	format_version: Schema.Literal(1),
	previous: Schema.optional(ToolchainGeneration),
});
export type ToolchainInstallationState = typeof ToolchainInstallationState.Type;

export class ToolchainStateError extends Data.TaggedError("ToolchainStateError")<{
	readonly cause: unknown;
	readonly engine_id: string;
	readonly operation: "read" | "write";
}> {}

/** Rejects values that could escape the per-engine layout before joining paths. */
export const ValidateToolchainPathComponent = (value: string) =>
	Schema.decodeUnknownEffect(SafeBasename)(value);

/**
 * The implicit profile an engine carries before anyone adds a second account.
 * Its home stays at the pre-profile path, so an existing managed install keeps
 * its sign-in without a migration step.
 */
export const default_toolchain_profile_id = "default";

/**
 * Pure path layout for one engine under the toolchain root.
 *
 * Binaries, install state, and staging are per engine; only the config home is
 * per profile. Profiles are therefore free: they share one verified executable
 * and differ solely in the `CLAUDE_CONFIG_DIR`/`CODEX_HOME` they are spawned
 * with, which is what lets two accounts of one engine run at the same time.
 */
export const make_toolchain_layout = (
	root: string,
	engine_id: string,
	path: Path.Path,
	profile_id: string = default_toolchain_profile_id,
) => {
	/** Engine IDs are catalog-owned, but retain this guard at the storage boundary. */
	if (!/^[A-Za-z0-9][A-Za-z0-9_-]*$/.test(engine_id))
		throw new Error(`Unsafe toolchain engine identifier: ${engine_id}`);
	/** Profile IDs reach this boundary from user-created profiles, so guard them too. */
	if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(profile_id))
		throw new Error(`Unsafe toolchain profile identifier: ${profile_id}`);
	const engine_root = path.join(root, engine_id);
	return {
		engine_root,
		profile_id,
		executable_path: (generation: ToolchainGeneration) =>
			path.join(engine_root, "versions", generation.directory, generation.binary),
		home_path:
			profile_id === default_toolchain_profile_id
				? path.join(engine_root, "home")
				: path.join(engine_root, "homes", profile_id),
		homes_path: path.join(engine_root, "homes"),
		staging_path: path.join(engine_root, "staging"),
		state_path: path.join(engine_root, "state.json"),
		generation_path: (directory: string) => path.join(engine_root, "versions", directory),
		versions_path: path.join(engine_root, "versions"),
	};
};
export type ToolchainLayout = ReturnType<typeof make_toolchain_layout>;

export const ReadToolchainState = (
	layout: ToolchainLayout,
	engine_id: string,
	file_system: FileSystem.FileSystem,
) =>
	Effect.gen(function* () {
		const exists = yield* file_system
			.exists(layout.state_path)
			.pipe(Effect.orElseSucceed(() => false));
		if (!exists) return undefined;
		const content = yield* file_system
			.readFileString(layout.state_path)
			.pipe(
				Effect.mapError(
					(cause) => new ToolchainStateError({ cause, engine_id, operation: "read" }),
				),
			);
		return yield* Schema.decodeUnknownEffect(Schema.fromJsonString(ToolchainInstallationState))(
			content,
		).pipe(
			Effect.mapError(
				(cause) => new ToolchainStateError({ cause, engine_id, operation: "read" }),
			),
		);
	});

/** Publishes the record through a sibling temp file so a crash never truncates it. */
export const WriteToolchainState = (
	layout: ToolchainLayout,
	engine_id: string,
	state: ToolchainInstallationState,
	file_system: FileSystem.FileSystem,
) =>
	Effect.gen(function* () {
		const temp_path = `${layout.state_path}.${randomUUID()}.tmp`;
		yield* file_system.makeDirectory(layout.engine_root, { recursive: true }).pipe(
			Effect.andThen(
				file_system.writeFileString(temp_path, `${JSON.stringify(state, null, "\t")}\n`),
			),
			Effect.andThen(file_system.rename(temp_path, layout.state_path)),
			Effect.mapError(
				(cause) => new ToolchainStateError({ cause, engine_id, operation: "write" }),
			),
		);
	});

/** Removes every version directory not referenced by the current record. */
export const PruneToolchainVersions = (
	layout: ToolchainLayout,
	state: ToolchainInstallationState,
	file_system: FileSystem.FileSystem,
	path: Path.Path,
) =>
	Effect.gen(function* () {
		const retained = new Set(
			[state.active.directory, state.previous?.directory].filter(
				(version): version is string => version !== undefined,
			),
		);
		const entries = yield* file_system
			.readDirectory(layout.versions_path)
			.pipe(Effect.orElseSucceed(() => []));
		for (const entry of entries) {
			/** Never use a directory entry as a path component without validation. */
			if (!Option.isSome(yield* ValidateToolchainPathComponent(entry).pipe(Effect.option)))
				continue;
			if (retained.has(entry)) continue;
			yield* file_system
				.remove(path.join(layout.versions_path, entry), { recursive: true })
				.pipe(Effect.ignore);
		}
	});
