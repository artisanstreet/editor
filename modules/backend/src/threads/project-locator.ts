import { realpath, stat } from "node:fs/promises";
import { basename, dirname, isAbsolute, normalize, resolve } from "node:path";

import { Context, Data, Effect, Layer, Option } from "effect";

import type { ProjectRef } from "@artisan/protocol";
import { ProcessRunner, type ProcessRunnerResult } from "../git/process-runner";
import { ProjectIdentityRegistry } from "../projects/project-identity-registry";

/** Identifies the ProjectLocator operation that could not complete. */
export type ProjectLocatorOperation = "discover" | "identify" | "normalize";

/** Identifies whether project ownership came from Git or a directory fallback. */
export type ProjectLocationSource = "directory" | "git_root";

/** Carries one canonical project and the evidence used to locate its root. */
export interface ProjectLocation {
	readonly project: ProjectRef;
	readonly source: ProjectLocationSource;
}

/** Reports a location-discovery failure without leaking platform-specific details. */
export class ProjectLocatorError extends Data.TaggedError("ProjectLocatorError")<{
	readonly cause: unknown;
	readonly location: string;
	readonly operation: ProjectLocatorOperation;
}> {}

/** Resolves a native working location into the project-safe protocol reference. */
export class ProjectLocator extends Context.Service<
	ProjectLocator,
	{
		readonly Locate: (
			location: string,
		) => Effect.Effect<Option.Option<ProjectLocation>, ProjectLocatorError>;
	}
>()("Artisan/ProjectLocator") {}

function project_locator_error(
	location: string,
	operation: ProjectLocatorOperation,
	cause: unknown,
) {
	return new ProjectLocatorError({ cause, location, operation });
}

function normalize_path(path: string) {
	const normalized_path = normalize(resolve(path)).replaceAll("\\", "/");

	return normalized_path.replace(/(?<!^[A-Za-z]:)\/$/, "");
}

function parse_git_root(result: ProcessRunnerResult, location: string) {
	if (result.exit_code !== 0) {
		return Effect.succeed(Option.none<string>());
	}

	if (result.stdout_truncated) {
		return Effect.fail(
			project_locator_error(
				location,
				"discover",
				new Error("Git worktree root output exceeded its configured byte limit"),
			),
		);
	}

	const output = new TextDecoder().decode(result.stdout);
	const root_path = output.replace(/\r?\n$/, "");

	if (
		!root_path ||
		root_path.includes("\0") ||
		/[\r\n]/.test(root_path) ||
		!isAbsolute(root_path)
	) {
		return Effect.fail(
			project_locator_error(
				location,
				"discover",
				new Error("Git worktree root output was not one absolute path"),
			),
		);
	}

	return Effect.succeed(Option.some(root_path));
}

function is_missing_path_error(cause: unknown) {
	if (!(cause instanceof Error) || !("code" in cause)) {
		return false;
	}

	return cause.code === "ENOENT" || cause.code === "ENOTDIR";
}

const ResolveLocation = (location: string) => {
	if (location.trim().length === 0) {
		return Effect.fail(
			project_locator_error(
				location,
				"normalize",
				new Error("Project location must not be empty"),
			),
		);
	}

	const ResolveCandidate = (candidate: string): Effect.Effect<string, ProjectLocatorError> =>
		Effect.tryPromise({
			try: () => realpath(candidate),
			catch: (cause) => cause,
		}).pipe(
			Effect.flatMap((canonical_path) =>
				Effect.tryPromise({
					try: () => stat(canonical_path),
					catch: (cause) => cause,
				}).pipe(
					Effect.map((metadata) =>
						metadata.isDirectory() ? canonical_path : dirname(canonical_path),
					),
				),
			),
			Effect.catch((cause) => {
				const parent = dirname(candidate);
				return is_missing_path_error(cause) && parent !== candidate
					? ResolveCandidate(parent)
					: Effect.fail(project_locator_error(location, "normalize", cause));
			}),
		);

	return ResolveCandidate(resolve(location));
};

const ResolveGitRoot = (location: string) =>
	Effect.gen(function* () {
		const canonical_path = yield* Effect.tryPromise({
			try: () => realpath(location),
			catch: (cause) => project_locator_error(location, "normalize", cause),
		});
		const metadata = yield* Effect.tryPromise({
			try: () => stat(canonical_path),
			catch: (cause) => project_locator_error(location, "normalize", cause),
		});
		if (!metadata.isDirectory())
			return yield* project_locator_error(
				location,
				"normalize",
				new Error("Git worktree root was not a directory"),
			);
		return canonical_path;
	});

/** Builds a Node locator that prefers Git worktree roots over local directories. */
export function make_node_project_locator_layer() {
	return Layer.effect(
		ProjectLocator,
		Effect.gen(function* () {
			const process_runner = yield* ProcessRunner;
			const identities = yield* ProjectIdentityRegistry;

			/**
			 * The id is minted state, not a hash of the path: the registry
			 * allocates a Snowflake on first sight and answers from storage
			 * forever after, so the same root always resolves to the same id.
			 */
			const MakeProjectRef = (
				location: string,
				root_path: string,
			): Effect.Effect<ProjectRef, ProjectLocatorError> =>
				Effect.gen(function* () {
					const normalized_root_path = normalize_path(root_path);
					const display_name = basename(normalized_root_path) || normalized_root_path;
					const project_id = yield* identities
						.Resolve(normalized_root_path)
						.pipe(
							Effect.mapError((cause) =>
								project_locator_error(location, "identify", cause),
							),
						);

					return { display_name, project_id, root_path: normalized_root_path };
				});

			const Locate = (location: string) =>
				Effect.gen(function* () {
					const directory_path = yield* ResolveLocation(location);
					const result = yield* process_runner
						.Run({
							args: ["rev-parse", "--path-format=absolute", "--show-toplevel"],
							command: "git",
							cwd: directory_path,
							max_stdout_bytes: 16 * 1024,
						})
						.pipe(
							Effect.mapError((cause) =>
								project_locator_error(location, "discover", cause),
							),
						);
					const git_root = yield* parse_git_root(result, location);
					const root_path = Option.isSome(git_root)
						? yield* ResolveGitRoot(git_root.value)
						: directory_path;

					return Option.some({
						project: yield* MakeProjectRef(location, root_path),
						source: Option.isSome(git_root) ? "git_root" : "directory",
					} satisfies ProjectLocation);
				});

			return { Locate };
		}),
	);
}

/** Supplies a locator with no known projects until desktop project discovery is composed. */
export const EmptyProjectLocatorLive = Layer.succeed(ProjectLocator, {
	Locate: () => Effect.succeed(Option.none()),
});
