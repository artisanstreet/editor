import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Effect, FileSystem, Layer, Option, Path, SynchronizedRef } from "effect";

/** Identifies the canonical executable pinned for one GitHub CLI service lifetime. */
export interface GitHubCliExecutableLocation {
	readonly path: string;
}

/** Resolves the GitHub CLI without invoking a shell or retaining a mutable PATH lookup. */
export class GitHubCliExecutable extends Context.Service<
	GitHubCliExecutable,
	{
		readonly Locate: Effect.Effect<Option.Option<GitHubCliExecutableLocation>>;
	}
>()("Artisan/GitHubCliExecutable") {}

/** Configures Node-backed GitHub CLI resolution. */
export interface NodeGitHubCliExecutableOptions {
	readonly command?: string;
	readonly cwd: string;
	readonly environment?: Readonly<Record<string, string | undefined>>;
	readonly platform?: NodeJS.Platform;
}

function environment_value(
	environment: Readonly<Record<string, string | undefined>>,
	name: string,
	platform: NodeJS.Platform,
) {
	if (platform !== "win32") {
		return environment[name];
	}

	const entry = Object.entries(environment).find(
		([key]) => key.toLowerCase() === name.toLowerCase(),
	);

	return entry?.[1];
}

function unquote_path_entry(entry: string) {
	return entry.length >= 2 && entry.startsWith('"') && entry.endsWith('"')
		? entry.slice(1, -1)
		: entry;
}

function windows_extensions(
	command: string,
	environment: Readonly<Record<string, string | undefined>>,
) {
	const extension = command.slice(command.lastIndexOf("."));

	if (extension !== command && extension.length > 1) {
		return [extension.toLowerCase()];
	}

	const configured = environment_value(environment, "PATHEXT", "win32") ?? ".COM;.EXE";

	return configured
		.split(";")
		.map((value) => value.trim().toLowerCase())
		.filter((value) => value === ".com" || value === ".exe");
}

function candidate_names(
	command: string,
	cwd: string,
	environment: Readonly<Record<string, string | undefined>>,
	platform: NodeJS.Platform,
	path_service: Path.Path,
) {
	const has_separator = command.includes("/") || (platform === "win32" && command.includes("\\"));
	const extensions = platform === "win32" ? windows_extensions(command, environment) : [""];
	const command_extension = path_service.extname(command).toLowerCase();

	if (
		platform === "win32" &&
		command_extension.length > 0 &&
		command_extension !== ".com" &&
		command_extension !== ".exe"
	) {
		return [];
	}

	const names =
		command_extension.length > 0 || platform !== "win32"
			? [command]
			: extensions.map((extension) => `${command}${extension}`);

	if (path_service.isAbsolute(command) || has_separator) {
		return names.map((name) => path_service.resolve(cwd, name));
	}

	const path_value = environment_value(environment, "PATH", platform) ?? "";
	const delimiter = platform === "win32" ? ";" : ":";
	const directories = path_value
		.split(delimiter)
		.map((entry) => unquote_path_entry(entry.trim()))
		.filter((entry) => entry.length > 0);

	return directories.flatMap((directory) =>
		names.map((name) => path_service.join(directory, name)),
	);
}

function ResolveCandidate(file_system: FileSystem.FileSystem, candidate: string) {
	return Effect.gen(function* () {
		const canonical = yield* file_system.realPath(candidate);
		const info = yield* file_system.stat(canonical);

		return info.type === "File"
			? Option.some({ path: canonical } satisfies GitHubCliExecutableLocation)
			: Option.none<GitHubCliExecutableLocation>();
	}).pipe(Effect.catch(() => Effect.succeed(Option.none<GitHubCliExecutableLocation>())));
}

function LocateExecutable(
	options: NodeGitHubCliExecutableOptions,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
) {
	return Effect.gen(function* () {
		const environment = options.environment ?? process.env;
		const platform = options.platform ?? process.platform;
		const command = options.command ?? "gh";
		const candidates = candidate_names(
			command,
			options.cwd,
			environment,
			platform,
			path_service,
		);
		const seen = new Set<string>();

		for (const candidate of candidates) {
			const key = platform === "win32" ? candidate.toLowerCase() : candidate;

			if (seen.has(key)) {
				continue;
			}

			seen.add(key);

			const location = yield* ResolveCandidate(file_system, candidate);

			if (Option.isSome(location)) {
				return location;
			}
		}

		return Option.none<GitHubCliExecutableLocation>();
	});
}

/** Builds a process-lifetime GitHub CLI executable pin with Effect filesystem services. */
export function make_node_github_cli_executable_layer(options: NodeGitHubCliExecutableOptions) {
	return Layer.effect(
		GitHubCliExecutable,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const pinned = yield* SynchronizedRef.make(Option.none<GitHubCliExecutableLocation>());
			const Locate = SynchronizedRef.modifyEffect(pinned, (current) =>
				Option.isSome(current)
					? Effect.succeed([current, current] as const)
					: LocateExecutable(options, file_system, path_service).pipe(
							Effect.map((located) => [located, located] as const),
						),
			);

			return { Locate };
		}),
	).pipe(Layer.provideMerge(NodeFileSystem.layer), Layer.provideMerge(NodePath.layer));
}
