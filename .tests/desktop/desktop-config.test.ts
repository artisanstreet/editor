import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { resolve_desktop_paths } from "@artisan/desktop";

const root = new URL("../..", import.meta.url);

describe("desktop packaging configuration", () => {
	it("uses explicit mutable and packaged paths", () => {
		expect(
			resolve_desktop_paths({
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			ae_command_path: "ae",
		});
	});

	it("resolves the packaged daemon independently from the Electron application archive", () => {
		expect(
			resolve_desktop_paths({
				is_packaged: true,
				resources_path: "C:/resources",
			}),
		).toMatchObject({
			ae_command_path: "C:\\resources\\artisan-forge\\ae.cmd",
		});
	});

	it("prefers the managed permanent ae supplied by the distribution launcher", () => {
		expect(
			resolve_desktop_paths({
				ae_command_override: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.cmd",
				is_packaged: true,
				resources_path: "C:/resources",
			}),
		).toEqual({
			ae_command_path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\ae.cmd",
		});
	});

	it("packages a sandboxed renderer host without any privileged bridge", () => {
		const config = readFileSync(new URL(".config/desktop-builder.yml", root), "utf8");
		const package_manifest = JSON.parse(
			readFileSync(new URL("package.json", root), "utf8"),
		) as { readonly scripts?: Record<string, string> };
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");
		const vite_config = readFileSync(new URL(".config/desktop.vite.config.ts", root), "utf8");

		expect(config).toContain("output: .dist/electron-release");
		expect(config).toContain("app: .dist/desktop");
		expect(config).toContain("electronLanguages: en-US");
		expect(config).toContain("- main.js");
		expect(config).toContain("- frontend/**");
		expect(config).not.toContain("preload.cjs");
		expect(config).not.toContain("asarUnpack:");
		expect(config).not.toContain("extraResources:");
		expect(config).not.toContain("from: .dist/forge");
		expect(config).not.toContain("nsis");
		expect(config).not.toContain("signExecutable: false");
		expect(config).toContain("- dir");
		expect(config).toContain("name: Artisan Forge");
		expect(config).toContain("- artisan");
		expect(config).not.toContain("from: .dist\n");
		expect(package_manifest.scripts?.["build:desktop"]).not.toContain("preload");
		expect(vite_config).not.toContain("node-pty");
		expect(vite_config).not.toContain("koffi");
		/** The staged renderer copy gets the loopback CSP variant; the browser copy stays same-origin. */
		expect(vite_config).toContain("connect-src 'self' http://127.0.0.1:*");
		expect(main).toContain("requestSingleInstanceLock");
		expect(main).not.toContain("setAsDefaultProtocolClient");
		/** The window is a pure renderer host: sandboxed, isolated, and bridge-free. */
		expect(main).toContain("BrowserWindow");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		expect(main).not.toContain("preload:");
		expect(main).not.toContain("ipcMain");
		/** Renderer drafts persist, while every pairing begins without a Forge cookie. */
		expect(main).toContain('const renderer_partition = "persist:artisan-renderer"');
		expect(main).toContain("partition: renderer_partition");
		expect(main).toContain('clearStorageData({ storages: ["cookies"] })');
		expect(main.match(/clearStorageData\(/g)).toHaveLength(1);
		expect(main).not.toContain("clearStorageData()");
		expect(main).toContain("desktop_lifecycle.Start()");
		expect(main).toContain("desktop_lifecycle.Reconnect()");
		expect(main).toContain('app.on("before-quit"');
		expect(main).toContain('const windows_app_user_model_id = "com.usebarekey.artisan-editor"');
		expect(main).toContain(
			'const windows_toast_activator_clsid = "{A7D8D3E7-9DE2-4C09-8D4B-4E490C20D3A4}"',
		);
		expect(main).toContain("app.setAppUserModelId(windows_app_user_model_id)");
		expect(main).toContain("app.setToastActivatorCLSID(windows_toast_activator_clsid)");
		expect(main.indexOf("app.setAppUserModelId(windows_app_user_model_id)")).toBeLessThan(
			main.indexOf("app.whenReady"),
		);
		expect(
			main.indexOf("app.setToastActivatorCLSID(windows_toast_activator_clsid)"),
		).toBeLessThan(main.indexOf("app.whenReady"));
		expect(main).toContain('process.platform === "win32"');
		expect(main).toContain('app.getPath("appData")');
		expect(main).toContain('"Artisan Editor.lnk"');
		expect(main).toContain("shell.readShortcutLink(shortcut_path)");
		expect(main).toContain('shell.writeShortcutLink(shortcut_path, "update"');
		expect(main).toContain("...current");
		expect(main).toContain("appUserModelId: windows_app_user_model_id");
		expect(main).toContain("toastActivatorClsid: windows_toast_activator_clsid");
		expect(main.indexOf("yield* RepairWindowsNotificationShortcut")).toBeLessThan(
			main.indexOf("new BrowserWindow"),
		);
		/** Pairing stays `ae`-owned: the editor only runs the hidden one-time handoff. */
		expect(main).toContain("make_node_forge_handoff_process_layer");
		expect(main).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(main).not.toContain("ARTISAN_DATABASE_PATH");
	});
});
