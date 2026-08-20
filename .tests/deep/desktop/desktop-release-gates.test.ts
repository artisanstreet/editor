import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../../..");
const frontend_root = resolve(workspace_root, "modules/frontend");
const frontend_config = readFileSync(resolve(frontend_root, "vite.config.ts"), "utf8");
const desktop_config = readFileSync(
	resolve(workspace_root, ".config/desktop.vite.config.ts"),
	"utf8",
);
const forge_config = readFileSync(
	resolve(workspace_root, ".config/forge.rolldown.config.ts"),
	"utf8",
);

const known_electron_package_files = [
	"electron-builder.yml",
	"electron-builder.yaml",
	".config/desktop-builder.yml",
	"electron-forge.config.ts",
	"forge.config.ts",
	"package.json",
] as const;

function has_electron_packaging_configuration() {
	return known_electron_package_files.some((file) => {
		const path = resolve(workspace_root, file);

		if (!existsSync(path)) {
			return false;
		}

		if (file === "package.json") {
			const manifest = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;

			return "build" in manifest || "config" in manifest;
		}

		return true;
	});
}

describe("deep desktop release gates", () => {
	it("keeps the renderer build static, strict, and isolated from a host bootstrap", () => {
		expect(frontend_config).toContain("adapter: adapter({");
		expect(frontend_config).toContain('assets: "../../.dist/frontend"');
		expect(frontend_config).toContain('pages: "../../.dist/frontend"');
		expect(frontend_config).toContain('fallback: "index.html"');
		expect(frontend_config).toContain("precompress: true");
		expect(frontend_config).toContain("strict: true");
		expect(frontend_config).not.toMatch(/\b(?:electron|@artisan\/backend)\b/);
	});

	it("packages a sandboxed renderer host around the ae-managed Forge", () => {
		const main = readFileSync(resolve(workspace_root, "modules/desktop/src/main.ts"), "utf8");
		const lifecycle = readFileSync(
			resolve(workspace_root, "modules/desktop/src/forge-handoff.ts"),
			"utf8",
		);

		expect(has_electron_packaging_configuration()).toBe(true);
		/** The editor renders the bundled frontend and pairs through `ae`'s one-time handoff. */
		expect(main).toContain("BrowserWindow");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		expect(main).not.toContain("preload:");
		expect(main).not.toContain("ipcMain");
		expect(main).toContain("make_node_forge_handoff_process_layer");
		expect(lifecycle).toContain('["open", "--handoff"]');
		expect(lifecycle).toContain("OwnedForgeStopArguments(instance_id)");
		expect(lifecycle).toContain("windowsHide: true");
		expect(main).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(main).not.toContain("ARTISAN_DATABASE_PATH");
		expect(lifecycle).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(lifecycle).not.toContain("ARTISAN_DATABASE_PATH");
		expect(desktop_config).not.toContain("koffi-win32-x64");
		expect(desktop_config).toContain("ssr: { noExternal: true }");
		/** The editor stages its own frontend copy with the loopback CSP variant. */
		expect(desktop_config).toContain('resolve(desktop_root, "frontend")');
		expect(desktop_config).toContain("connect-src 'self' http://127.0.0.1:*");
		expect(forge_config).toContain("koffi-win32-x64");
		expect(forge_config).toContain('"ae.cmd"');
		expect(forge_config).toContain("ARTISAN_NATIVE_RUNTIME=%~dp0native-runtime");
		const builder = readFileSync(
			resolve(workspace_root, ".config/desktop-builder.yml"),
			"utf8",
		);
		expect(builder).toContain("- dir");
		expect(builder).toContain("- frontend/**");
		expect(builder).not.toContain("nsis");
		expect(builder).not.toContain("signExecutable: false");
		expect(builder).not.toContain("extraResources:");
	});

	it("verifies the installed ae payload and ownership boundary locally", () => {
		const verifier = readFileSync(
			resolve(workspace_root, ".tests/deep/desktop/verify-packaged-desktop.ts"),
			"utf8",
		);
		expect(verifier).toContain("embedded_forge");
		expect(verifier).toContain("must not embed a parallel Forge lifecycle");
		expect(verifier).toContain("Packaged desktop renderer evidence");
		/** The verifier now proves the honest renderer shape, not a launcher-only ASAR. */
		expect(verifier).toContain('"/frontend/index.html"');
		expect(verifier).toContain("loopback Forge CSP allowance");
		expect(verifier).toContain('"/preload.cjs"');
		expect(verifier).not.toContain("ARTISAN_PACKAGED_SMOKE");
		expect(verifier).not.toContain("Stop-Process");
	});
});
