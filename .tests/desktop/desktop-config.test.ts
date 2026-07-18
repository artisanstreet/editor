import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { resolve_desktop_paths, resolve_frontend_request } from "@artisan/desktop";

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
			frontend_index_path: "C:\\app\\.dist\\frontend\\index.html",
			migrations_path: "C:\\app\\modules\\backend\\drizzle",
		});
	});

	it("confines custom-protocol assets below the packaged root", () => {
		expect(resolve_frontend_request("C:/resources/.dist/frontend", "artisan://app/index.html")).toBe(
			"C:\\resources\\.dist\\frontend\\index.html",
		);
		expect(resolve_frontend_request("C:/resources/.dist/frontend", "artisan://app/%2e%2e/secret")).toBeUndefined();
		expect(resolve_frontend_request("C:/resources/.dist/frontend", "artisan://other/index.html")).toBeUndefined();
	});

	it("keeps native modules unpacked and exposes only the narrow preload bridge", () => {
		const config = readFileSync(new URL("desktop-builder.yml", root), "utf8");
		const main = readFileSync(new URL("modules/desktop/src/main.ts", root), "utf8");
		const preload = readFileSync(new URL("modules/desktop/src/preload.ts", root), "utf8");

		expect(config).toContain("**/*.node");
		expect(config).toContain("**/node-pty/**");
		expect(main).toContain("requestSingleInstanceLock");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		expect(main).toContain("protocol.handle");
		expect(preload).toContain("requestConnection");
		expect(preload).not.toContain("ipcRenderer.send");
	});
});
