import { Config, Context, Data, Effect, Layer, Option, Path, Schema } from "effect";
import { ChildProcess } from "effect/unstable/process";
import { ChildProcessSpawner } from "effect/unstable/process/ChildProcessSpawner";

import { ForgeProfileName } from "./profile";

export type CliPlatformKind = "darwin" | "linux" | "win32";

export class CliPlatform extends Context.Service<
	CliPlatform,
	{
		readonly cli_entry_path: Option.Option<string>;
		readonly executable_path: string;
		readonly home: string;
		readonly kind: CliPlatformKind;
	}
>()("Artisan/CliPlatform") {}

const OptionalConfig = (name: string) => Config.string(name).pipe(Config.option);

/** Host globals are read only while constructing the executable's live adapter. */
export const make_cli_platform_layer = (
	host: {
		readonly argv_path: string | undefined;
		readonly cwd: string;
		readonly executable_path: string;
		readonly platform: NodeJS.Platform;
	} = {
		argv_path: process.argv[1],
		cwd: process.cwd(),
		executable_path: process.execPath,
		platform: process.platform,
	},
) =>
	Layer.effect(
		CliPlatform,
		Effect.gen(function* () {
			const path = yield* Path.Path;
			const [artisan_home, local_app_data, user_profile, state_home, unix_home] =
				yield* Effect.all([
					OptionalConfig("ARTISAN_HOME"),
					OptionalConfig("LOCALAPPDATA"),
					OptionalConfig("USERPROFILE"),
					OptionalConfig("XDG_STATE_HOME"),
					OptionalConfig("HOME"),
				]);
			const executable_override = yield* OptionalConfig("ARTISAN_AE_RUNTIME");
			const entry_override = yield* OptionalConfig("ARTISAN_AE_ENTRY");
			const kind: CliPlatformKind =
				host.platform === "win32"
					? "win32"
					: host.platform === "darwin"
						? "darwin"
						: "linux";
			const fallback_home =
				kind === "win32"
					? path.join(
							Option.getOrElse(local_app_data, () =>
								Option.getOrElse(user_profile, () => host.cwd),
							),
							"Artisan",
						)
					: path.join(
							Option.getOrElse(state_home, () =>
								path.join(
									Option.getOrElse(unix_home, () => host.cwd),
									".local",
									"state",
								),
							),
							"artisan",
						);
			return CliPlatform.of({
				cli_entry_path: Option.orElse(entry_override, () =>
					Option.fromNullishOr(host.argv_path),
				),
				executable_path: Option.getOrElse(executable_override, () => host.executable_path),
				home: Option.getOrElse(artisan_home, () => fallback_home),
				kind,
			});
		}),
	);

export class ForgeAutostartError extends Data.TaggedError("ForgeAutostartError")<{
	readonly code: "unsupported" | "failed";
	readonly cause?: unknown;
}> {}

export class ForgeAutostart extends Context.Service<
	ForgeAutostart,
	{
		readonly Configure: (input: {
			readonly enabled: boolean;
			readonly profile: string;
		}) => Effect.Effect<
			{ readonly state: "enabled" | "disabled" | "unsupported" },
			ForgeAutostartError,
			ChildProcessSpawner
		>;
		readonly Status: (input: {
			readonly profile: string;
		}) => Effect.Effect<
			{ readonly state: "enabled" | "disabled" | "unsupported" },
			ForgeAutostartError,
			ChildProcessSpawner
		>;
	}
>()("Artisan/ForgeAutostart") {}

const valid_profile = Schema.is(ForgeProfileName);

/** The task name is deliberately profile-scoped and cannot name a system task. */
export const ForgeTaskName = (profile: string) => `Artisan Forge ${profile}`;

export class BrowserOpener extends Context.Service<
	BrowserOpener,
	{
		readonly Open: (
			url: string,
		) => Effect.Effect<void, ForgeAutostartError, ChildProcessSpawner>;
	}
>()("Artisan/BrowserOpener") {}

/** Uses argv only: no shell interpolation or capability-bearing command line. */
export const make_browser_opener_layer = Layer.effect(
	BrowserOpener,
	Effect.gen(function* () {
		const platform = yield* CliPlatform;
		return BrowserOpener.of({
			Open: (url) => {
				const [executable, args] =
					platform.kind === "win32"
						? ["explorer.exe", [url]]
						: platform.kind === "darwin"
							? ["open", [url]]
							: ["xdg-open", [url]];
				return Effect.scoped(
					ChildProcess.make(executable, args, {
						detached: true,
						stderr: "ignore",
						stdin: "ignore",
						stdout: "ignore",
					}).pipe(
						Effect.asVoid,
						Effect.mapError(
							(cause) => new ForgeAutostartError({ cause, code: "failed" }),
						),
					),
				);
			},
		});
	}),
);

/** Current-user only Task Scheduler arguments; execution remains an injected adapter in tests. */
export const task_scheduler_arguments = (input: {
	readonly cli_entry_path?: string;
	readonly enabled: boolean;
	readonly executable_path: string;
	readonly profile: string;
}) => {
	const launch =
		input.cli_entry_path === undefined
			? `"${input.executable_path}" start --profile ${input.profile}`
			: `"${input.executable_path}" "${input.cli_entry_path}" start --profile ${input.profile}`;
	return input.enabled
		? [
				"/Create",
				"/F",
				"/SC",
				"ONLOGON",
				"/RL",
				"LIMITED",
				"/TN",
				ForgeTaskName(input.profile),
				"/TR",
				launch,
			]
		: ["/Delete", "/F", "/TN", ForgeTaskName(input.profile)];
};

/**
 * Current-user Task Scheduler integration. `schtasks` is launched through the
 * Effect process service with a pre-split argv, never through cmd or a shell.
 */
export const make_windows_autostart_layer = (
	options: {
		readonly cli_entry_path?: string;
		readonly executable_path?: string;
	} = {},
) =>
	Layer.effect(
		ForgeAutostart,
		Effect.gen(function* () {
			const platform = yield* CliPlatform;
			const executable_path = options.executable_path ?? platform.executable_path;
			const cli_entry_path =
				options.cli_entry_path ?? Option.getOrUndefined(platform.cli_entry_path);
			return ForgeAutostart.of({
				Configure: ({ enabled, profile }) => {
					if (platform.kind !== "win32")
						return Effect.succeed({ state: "unsupported" as const });
					if (!valid_profile(profile))
						return Effect.fail(new ForgeAutostartError({ code: "failed" }));
					return Effect.scoped(
						ChildProcess.make(
							"schtasks.exe",
							task_scheduler_arguments({
								...(cli_entry_path === undefined ? {} : { cli_entry_path }),
								enabled,
								executable_path,
								profile,
							}),
							{ stderr: "ignore", stdin: "ignore", stdout: "ignore" },
						).pipe(
							Effect.flatMap((child) => child.exitCode),
							Effect.flatMap((code) =>
								code === 0
									? Effect.succeed({
											state: enabled
												? ("enabled" as const)
												: ("disabled" as const),
										})
									: Effect.fail(new ForgeAutostartError({ code: "failed" })),
							),
							Effect.mapError((cause) =>
								cause instanceof ForgeAutostartError
									? cause
									: new ForgeAutostartError({ cause, code: "failed" }),
							),
						),
					);
				},
				Status: ({ profile }) => {
					if (platform.kind !== "win32")
						return Effect.succeed({ state: "unsupported" as const });
					if (!valid_profile(profile))
						return Effect.fail(new ForgeAutostartError({ code: "failed" }));
					return Effect.scoped(
						ChildProcess.make(
							"schtasks.exe",
							["/Query", "/TN", ForgeTaskName(profile)],
							{
								stderr: "ignore",
								stdin: "ignore",
								stdout: "ignore",
							},
						).pipe(
							Effect.flatMap((child) => child.exitCode),
							Effect.map((code) => ({
								state: code === 0 ? ("enabled" as const) : ("disabled" as const),
							})),
							Effect.mapError(
								(cause) => new ForgeAutostartError({ cause, code: "failed" }),
							),
						),
					);
				},
			});
		}),
	);

export const make_unsupported_autostart_layer = Layer.succeed(
	ForgeAutostart,
	ForgeAutostart.of({
		Configure: () => Effect.succeed({ state: "unsupported" as const }),
		Status: () => Effect.succeed({ state: "unsupported" as const }),
	}),
);
