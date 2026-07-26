import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { resolve_desktop_paths } from "@artisan/desktop";

const root = new URL("../..", import.meta.url);

describe("desktop packaging configuration", () => {
	it("uses explicit mutable and packaged paths", () => {
		expect(
			resolve_desktop_paths({
				app_data_path: "C:/Users/user/AppData",
				app_root_path: "C:/app",
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			database_path: "C:\\Users\\user\\AppData\\Artisan Editor\\artisan.sqlite",
			forge_entry_path: "C:\\app\\.dist\\forge\\host.js",
			forge_executable_path: "C:\\app\\.dist\\forge\\Artisan Forge.exe",
			forge_native_runtime_path: "C:\\app\\.dist\\forge\\native-runtime",
			forge_node_executable_path: "C:\\app\\.dist\\forge\\node.exe",
			preload_path: "C:\\app\\.dist\\desktop\\preload.cjs",
		});
	});

	it("resolves the packaged daemon independently from the Electron application archive", () => {
		expect(
			resolve_desktop_paths({
				app_data_path: "C:/Users/user/AppData",
				app_root_path: "C:/resources/app.asar",
				is_packaged: true,
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			forge_entry_path: "C:\\resources\\artisan-forge\\host.js",
			forge_executable_path: "C:\\resources\\artisan-forge\\Artisan Forge.exe",
			forge_native_runtime_path: "C:\\resources\\artisan-forge\\native-runtime",
			forge_node_executable_path: "C:\\resources\\artisan-forge\\node.exe",
			preload_path: "C:\\resources\\app.asar\\preload.cjs",
		});
	});

	it("keeps native modules unpacked and exposes only the narrow preload bridge", () => {
		const config = readFileSync(new URL("desktop-builder.yml", root), "utf8");
		const package_manifest = JSON.parse(
			readFileSync(new URL("package.json", root), "utf8"),
		) as { readonly scripts?: Record<string, string> };
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");
		const preload = readFileSync(new URL("modules/desktop/src/preload.ts", root), "utf8");
		const vite_config = readFileSync(new URL("desktop.vite.config.ts", root), "utf8");
		const forge_supervisor = readFileSync(
			new URL("modules/desktop/src/forge-process-supervisor.ts", root),
			"utf8",
		);
		const window_activity = readFileSync(
			new URL("modules/desktop/src/window-activity.ts", root),
			"utf8",
		);

		expect(config).toContain("**/*.node");
		expect(config).toContain("output: .dist/electron-release");
		expect(config).toContain("app: .dist/desktop");
		expect(config).toContain("- main.js");
		expect(config).toContain("- preload.cjs");
		expect(config).toContain("from: .dist/forge");
		expect(config).not.toContain("from: .dist\n");
		expect(package_manifest.scripts?.["build:desktop"]).toContain(
			"desktop.preload.vite.config.ts",
		);
		expect(vite_config).toContain(
			'"node-pty": resolve(import.meta.dirname, "modules/desktop/src/node-pty-shim.ts")',
		);
		expect(forge_supervisor).toContain("forge_native_runtime_path");
		expect(forge_supervisor).toContain("DesktopProcessEnvironment");
		expect(forge_supervisor).toContain("environment.inherited.NODE_PATH");
		expect(forge_supervisor).not.toContain("process.env.NODE_PATH");
		expect(forge_supervisor).toContain("Effect.retry(RestartSchedule)");
		expect(forge_supervisor).toContain("/api/pair/request");
		expect(forge_supervisor).not.toContain("searchParams.set");
		expect(main).toContain("requestSingleInstanceLock");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		expect(main).toContain("window.loadURL(forge_http_endpoint.toString())");
		expect(main).not.toContain("protocol.handle");
		expect(main).toContain('app.on("activate"');
		expect(main).toContain("before-quit");
		expect(preload).not.toContain("requestConnection");
		expect(preload).toContain("forgeWebSocketEndpoint");
		expect(preload).toContain("identity");
		expect(preload).toContain("selectProjectDirectory");
		expect(preload).toContain("setWorking");
		expect(main).toContain("read_desktop_identity");
		expect(main).toContain("mainFrame.ipc.handle(identity_channel");
		expect(main).toContain("mainFrame.ipc.handle(activity_channel");
		expect(main).toContain("mainFrame.ipc.handle(project_picker_channel");
		expect(window_activity).toContain('setProgressBar(2, { mode: "indeterminate" })');
		expect(main).toContain("activity.RestoreIdle");
		expect(preload).not.toContain("ipcRenderer.send");
	});
});
