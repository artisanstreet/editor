import { spawn } from "node:child_process";

import { app } from "electron";
import { Data, Effect, Option } from "effect";

import { FindForgeStartLaunchRequest } from "./launch-request";
import { resolve_desktop_paths } from "./paths";

export class DesktopLauncherError extends Data.TaggedError("DesktopLauncherError")<{
	readonly cause?: unknown;
	readonly command: "open" | "start";
	readonly exit_code?: number;
}> {}

const RunAe = (ae_command_path: string, command: "open" | "start") =>
	Effect.callback<void, DesktopLauncherError>((resume) => {
		const child = spawn(ae_command_path, [command], {
			shell: process.platform === "win32",
			stdio: "ignore",
			windowsHide: true,
		});
		let settled = false;
		const complete = (result: Effect.Effect<void, DesktopLauncherError>) => {
			if (settled) return;
			settled = true;
			child.off("error", on_error);
			child.off("exit", on_exit);
			resume(result);
		};
		const on_error = (cause: Error) =>
			complete(Effect.fail(new DesktopLauncherError({ cause, command })));
		const on_exit = (exit_code: number | null) =>
			complete(
				exit_code === 0
					? Effect.void
					: Effect.fail(
							new DesktopLauncherError({
								command,
								...(exit_code === null ? {} : { exit_code }),
							}),
						),
			);
		child.once("error", on_error);
		child.once("exit", on_exit);

		return Effect.sync(() => {
			settled = true;
			child.off("error", on_error);
			child.off("exit", on_exit);
		});
	});

/**
 * Electron is only a registered native launcher. `ae` owns Forge lifecycle and pairing;
 * this process never reads profile state, handles credentials, or owns a Forge child.
 */
export const StartDesktop = Effect.gen(function* () {
	const initial_request = FindForgeStartLaunchRequest(process.argv);
	if (!app.requestSingleInstanceLock()) {
		app.quit();
		return;
	}

	yield* Effect.tryPromise({
		try: () => app.whenReady(),
		catch: (cause) => cause,
	});
	const paths = resolve_desktop_paths({
		...(process.env.ARTISAN_AE_COMMAND === undefined
			? {}
			: { ae_command_override: process.env.ARTISAN_AE_COMMAND }),
		is_packaged: app.isPackaged,
		resources_path: process.resourcesPath,
	});
	const OpenForge = RunAe(paths.ae_command_path, "open");
	const StartAndOpenForge = RunAe(paths.ae_command_path, "start").pipe(Effect.andThen(OpenForge));

	const launch = Option.isSome(initial_request) ? StartAndOpenForge : OpenForge;
	yield* launch;
	app.quit();
});

/** The sole Desktop Effect runtime bootstrap. */
void Effect.runPromise(StartDesktop).catch((cause) => {
	console.error(
		JSON.stringify({
			kind: "artisan:desktop-launch",
			message: cause instanceof Error ? cause.message : "Unable to open Artisan Forge",
			ok: false,
		}),
	);
	app.exit(1);
});
