import { spawn, type ChildProcess, type SpawnOptions } from "node:child_process";

type EditorLauncherProcess = Pick<ChildProcess, "once">;

export type SpawnEditorLauncher = (
	executable: string,
	arguments_: ReadonlyArray<string>,
	options: SpawnOptions,
) => EditorLauncherProcess;

const spawn_editor_launcher: SpawnEditorLauncher = (executable, arguments_, options) =>
	spawn(executable, [...arguments_], options);

export const editor_launch_options = {
	detached: true,
	stdio: "ignore",
	windowsHide: true,
} as const satisfies SpawnOptions;

/**
 * The release build owns only the launch attempt, not the installed editor's
 * lifetime. Its stdio must not be piped: the first Electron process can retain
 * those handles after `ae` exits and leave an otherwise-finished build open.
 */
export const launch_installed_editor = (
	executable: string,
	spawn_launcher: SpawnEditorLauncher = spawn_editor_launcher,
): Promise<void> =>
	new Promise((resolve, reject) => {
		const child = spawn_launcher(executable, ["open"], editor_launch_options);

		child.once("error", reject);
		child.once("exit", (code, signal) => {
			if (code === 0) {
				resolve();
				return;
			}

			reject(
				new Error(
					code === null
						? `ae open was interrupted by ${signal ?? "an unknown signal"}`
						: `ae open exited with code ${code}`,
				),
			);
		});
	});
