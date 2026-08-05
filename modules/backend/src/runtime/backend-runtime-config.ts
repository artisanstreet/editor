import { Config, Data, Effect, FileSystem, Option, Path } from "effect";

export class BackendRuntimeConfigurationError extends Data.TaggedError(
	"BackendRuntimeConfigurationError",
)<{
	readonly message: string;
}> {}

export interface BackendRuntimePlatformOptions {
	readonly claude_home?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
}

export interface BackendRuntimeConfiguration {
	readonly claude_home: string;
	readonly codex_home: string;
	readonly current_directory: string;
	readonly home_directory: string;
}

const OptionalConfig = (name: string) => Config.string(name).pipe(Config.option);

/** Resolves ambient backend paths through Effect configuration and platform services. */
export const ResolveBackendRuntimeConfiguration = (options: BackendRuntimePlatformOptions = {}) =>
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const path = yield* Path.Path;
		const [configured_user_profile, configured_home_directory] = yield* Effect.all([
			OptionalConfig("USERPROFILE"),
			OptionalConfig("HOME"),
		]);
		const configured_codex_home = yield* OptionalConfig("CODEX_HOME");
		const configured_claude_home = yield* OptionalConfig("CLAUDE_CONFIG_DIR");
		const home_directory =
			options.home_directory ??
			Option.getOrUndefined(configured_user_profile) ??
			Option.getOrUndefined(configured_home_directory);

		if (home_directory === undefined || home_directory.trim().length === 0) {
			return yield* new BackendRuntimeConfigurationError({
				message: "The backend home directory could not be resolved.",
			});
		}

		const current_directory = path.resolve(".");
		yield* file_system.exists(current_directory);

		/**
		 * An explicitly supplied home directory pins every harness path beneath
		 * it, so an ambient `CODEX_HOME` or `CLAUDE_CONFIG_DIR` cannot pull a
		 * test or a sandboxed runtime back out to the real user profile.
		 */
		return {
			claude_home:
				options.claude_home ??
				(options.home_directory === undefined
					? Option.getOrUndefined(configured_claude_home)
					: undefined) ??
				path.join(home_directory, ".claude"),
			codex_home:
				options.codex_home ??
				(options.home_directory === undefined
					? Option.getOrUndefined(configured_codex_home)
					: undefined) ??
				path.join(home_directory, ".codex"),
			current_directory,
			home_directory,
		} satisfies BackendRuntimeConfiguration;
	});

export class DesktopEngineConfigurationError extends Data.TaggedError(
	"DesktopEngineConfigurationError",
)<{
	readonly message: string;
}> {}
