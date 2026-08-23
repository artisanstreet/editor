import { EventEmitter } from "node:events";

import { describe, expect, it, vi } from "vitest";

import {
	editor_launch_options,
	launch_installed_editor,
	type SpawnEditorLauncher,
} from "../../.scripts/build/launch-editor";

class FakeLauncher extends EventEmitter {}

describe("release build editor launch", () => {
	it("waits for a successful detached ae launcher without piping the editor", async () => {
		const child = new FakeLauncher();
		const spawn_launcher = vi.fn(() => child) as unknown as SpawnEditorLauncher;
		const launched = launch_installed_editor("C:\\Artisan\\bin\\ae.exe", spawn_launcher);

		expect(spawn_launcher).toHaveBeenCalledWith(
			"C:\\Artisan\\bin\\ae.exe",
			["open"],
			editor_launch_options,
		);
		expect(editor_launch_options).toMatchObject({
			detached: true,
			stdio: "ignore",
			windowsHide: true,
		});

		child.emit("exit", 0, null);

		await expect(launched).resolves.toBeUndefined();
	});

	it("still fails the build when the launcher cannot start", async () => {
		const child = new FakeLauncher();
		const spawn_launcher = (() => child) as unknown as SpawnEditorLauncher;
		const launched = launch_installed_editor("missing-ae.exe", spawn_launcher);
		const failure = new Error("spawn ENOENT");

		child.emit("error", failure);

		await expect(launched).rejects.toBe(failure);
	});

	it("keeps a real ae launch failure visible to the checklist", async () => {
		const child = new FakeLauncher();
		const spawn_launcher = (() => child) as unknown as SpawnEditorLauncher;
		const launched = launch_installed_editor("C:\\Artisan\\bin\\ae.exe", spawn_launcher);

		child.emit("exit", 7, null);

		await expect(launched).rejects.toThrow("ae open exited with code 7");
	});
});
