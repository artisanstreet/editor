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

	it("packages only the native protocol launcher for managed distribution", () => {
		const config = readFileSync(new URL("desktop-builder.yml", root), "utf8");
		const package_manifest = JSON.parse(
			readFileSync(new URL("package.json", root), "utf8"),
		) as { readonly scripts?: Record<string, string> };
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");
		const vite_config = readFileSync(new URL("desktop.vite.config.ts", root), "utf8");

		expect(config).toContain("output: .dist/electron-release");
		expect(config).toContain("app: .dist/desktop");
		expect(config).toContain("- main.js");
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
		expect(main).toContain("requestSingleInstanceLock");
		expect(main).not.toContain("setAsDefaultProtocolClient");
		expect(main).toContain('RunAe(paths.ae_command_path, "open")');
		expect(main).toContain('RunAe(paths.ae_command_path, "start")');
		expect(main).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(main).not.toContain("ARTISAN_DATABASE_PATH");
		expect(main).not.toContain("BrowserWindow");
	});
});
