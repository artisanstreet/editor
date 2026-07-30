import { homedir } from "node:os";
import { delimiter, join } from "node:path";

import { Context, Effect, FileSystem, Layer } from "effect";

/** Supplies the host paths and inherited environment used by Codex process composition. */
export class CodexRuntimeEnvironment extends Context.Service<
	CodexRuntimeEnvironment,
	{
		readonly architecture: string;
		readonly inherited_environment: Readonly<NodeJS.ProcessEnv>;
		readonly local_app_data: string;
		readonly platform: NodeJS.Platform;
		readonly user_profile: string;
	}
>()("Artisan/CodexRuntimeEnvironment") {}

/** Acquires Node host state once at the live platform boundary. */
export const CodexRuntimeEnvironmentLive = Layer.sync(CodexRuntimeEnvironment, () => {
	const inherited_environment = { ...process.env };
	const user_profile = inherited_environment.USERPROFILE?.trim() || homedir();

	return {
		architecture: process.arch,
		inherited_environment,
		local_app_data:
			inherited_environment.LOCALAPPDATA?.trim() || join(user_profile, "AppData", "Local"),
		platform: process.platform,
		user_profile,
	};
});

export interface CodexExecutableResolverInput {
	readonly architecture?: string;
	readonly environment?: Readonly<NodeJS.ProcessEnv>;
	readonly Exists?: (path: string) => boolean;
	readonly local_app_data?: string;
	readonly platform?: NodeJS.Platform;
	readonly ReadDirectory?: (path: string) => ReadonlyArray<string>;
}

const IsWindowsAppsPath = (path: string, local_app_data: string | undefined) =>
	local_app_data !== undefined &&
	path
		.toLocaleLowerCase()
		.startsWith(join(local_app_data, "Microsoft", "WindowsApps").toLocaleLowerCase());

const CompareCodexDirectoryNames = (left: string, right: string) =>
	left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });

/**
 * Resolves a directly executable Codex CLI without falling through to Windows'
 * inaccessible App Execution Alias.
 */
export const resolve_codex_executable = (input: CodexExecutableResolverInput = {}) => {
	const environment = input.environment ?? {};
	const configured_executable = environment.ARTISAN_CODEX_EXECUTABLE?.trim();
	const platform = input.platform ?? "linux";
	const architecture = input.architecture ?? "x64";
	const Exists = input.Exists ?? (() => false);
	const ReadDirectory = input.ReadDirectory ?? (() => []);

	if (configured_executable !== undefined && configured_executable.length > 0) {
		return configured_executable;
	}

	if (platform !== "win32") return "codex";

	const local_app_data = environment.LOCALAPPDATA?.trim() || input.local_app_data;
	const local_codex_root = join(local_app_data ?? "", "OpenAI", "Codex", "bin");
	const winget_executable =
		local_app_data === undefined
			? undefined
			: join(
					local_app_data,
					"Microsoft",
					"WinGet",
					"Packages",
					"OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe",
					`codex-${architecture === "arm64" ? "aarch64" : "x86_64"}-pc-windows-msvc.exe`,
				);
	const local_candidates = [
		join(local_codex_root, "codex.exe"),
		...[...ReadDirectory(local_codex_root)]
			.sort(CompareCodexDirectoryNames)
			.reverse()
			.map((directory) => join(local_codex_root, directory, "codex.exe")),
	];
	const path_candidates = (environment.PATH ?? "")
		.split(delimiter)
		.filter((path) => path.length > 0 && !IsWindowsAppsPath(path, local_app_data))
		.map((path) => join(path, "codex.exe"));

	return (
		[
			...local_candidates,
			...(winget_executable === undefined ? [] : [winget_executable]),
			...path_candidates,
		].find(Exists) ?? join(local_codex_root, "codex.exe")
	);
};

/** Supplies every Codex subprocess with the user's ordinary Codex home. */
export const make_codex_process_environment = (
	environment: NodeJS.ProcessEnv = {},
	inherited_environment: Readonly<NodeJS.ProcessEnv> = {},
	user_profile = inherited_environment.USERPROFILE?.trim() ?? "",
) => {
	const resolved_environment = { ...inherited_environment, ...environment };

	if (resolved_environment.CODEX_HOME?.trim()) return resolved_environment;

	return {
		...resolved_environment,
		CODEX_HOME: join(resolved_environment.USERPROFILE?.trim() || user_profile, ".codex"),
	};
};

/** Resolves the executable through Effect's filesystem and runtime services. */
export const ResolveCodexExecutable = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const runtime_environment = yield* CodexRuntimeEnvironment;
	const local_codex_root = join(runtime_environment.local_app_data, "OpenAI", "Codex", "bin");
	const directory_names = yield* file_system
		.readDirectory(local_codex_root)
		.pipe(Effect.orElseSucceed(() => []));
	const fallback = resolve_codex_executable({
		architecture: runtime_environment.architecture,
		environment: runtime_environment.inherited_environment,
		Exists: () => false,
		local_app_data: runtime_environment.local_app_data,
		platform: runtime_environment.platform,
		ReadDirectory: () => directory_names,
	});

	if (runtime_environment.platform !== "win32") return fallback;

	const environment = runtime_environment.inherited_environment;
	const configured_executable = environment.ARTISAN_CODEX_EXECUTABLE?.trim();
	if (configured_executable) return configured_executable;

	const local_app_data = environment.LOCALAPPDATA?.trim() || runtime_environment.local_app_data;
	const local_candidates = [
		join(local_app_data, "OpenAI", "Codex", "bin", "codex.exe"),
		...directory_names
			.sort(CompareCodexDirectoryNames)
			.reverse()
			.map((directory) =>
				join(local_app_data, "OpenAI", "Codex", "bin", directory, "codex.exe"),
			),
		join(
			local_app_data,
			"Microsoft",
			"WinGet",
			"Packages",
			"OpenAI.Codex_Microsoft.Winget.Source_8wekyb3d8bbwe",
			`codex-${runtime_environment.architecture === "arm64" ? "aarch64" : "x86_64"}-pc-windows-msvc.exe`,
		),
		...(environment.PATH ?? "")
			.split(delimiter)
			.filter((path) => path.length > 0 && !IsWindowsAppsPath(path, local_app_data))
			.map((path) => join(path, "codex.exe")),
	];

	for (const candidate of local_candidates) {
		if (yield* file_system.exists(candidate).pipe(Effect.orElseSucceed(() => false))) {
			return candidate;
		}
	}

	return fallback;
});
