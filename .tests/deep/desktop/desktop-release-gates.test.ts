import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../../..");
const frontend_root = resolve(workspace_root, "modules/frontend");
const frontend_config = readFileSync(resolve(frontend_root, "vite.config.ts"), "utf8");
const desktop_config = readFileSync(resolve(workspace_root, "desktop.vite.config.ts"), "utf8");
const forge_config = readFileSync(resolve(workspace_root, "forge.vite.config.ts"), "utf8");
const release_policy = readFileSync(
	resolve(workspace_root, "docs/release/validation-policy.md"),
	"utf8",
);

const known_electron_package_files = [
	"electron-builder.yml",
	"electron-builder.yaml",
	"desktop-builder.yml",
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

		expect(has_electron_packaging_configuration()).toBe(true);
		/** The editor renders the bundled frontend and pairs through `ae`'s one-time handoff. */
		expect(main).toContain("BrowserWindow");
		expect(main).toContain("contextIsolation: true");
		expect(main).toContain("nodeIntegration: false");
		expect(main).toContain("sandbox: true");
		expect(main).not.toContain("preload:");
		expect(main).not.toContain("ipcMain");
		expect(main).toContain('"open", "--handoff"');
		expect(main).not.toContain("ARTISAN_AUTH_TOKEN");
		expect(main).not.toContain("ARTISAN_DATABASE_PATH");
		expect(desktop_config).not.toContain("koffi-win32-x64");
		expect(desktop_config).toContain("ssr: { noExternal: true }");
		/** The editor stages its own frontend copy with the loopback CSP variant. */
		expect(desktop_config).toContain('resolve(desktop_root, "frontend")');
		expect(desktop_config).toContain("connect-src 'self' http://127.0.0.1:*");
		expect(forge_config).toContain("koffi-win32-x64");
		expect(forge_config).toContain('"ae.cmd"');
		expect(forge_config).toContain("ARTISAN_NATIVE_RUNTIME=%~dp0native-runtime");
		expect(forge_config).toContain('"update-user-path.ps1"');
		const builder = readFileSync(resolve(workspace_root, "desktop-builder.yml"), "utf8");
		expect(builder).toContain("- dir");
		expect(builder).toContain("- frontend/**");
		expect(builder).not.toContain("nsis");
		expect(builder).not.toContain("signExecutable: false");
		expect(builder).not.toContain("extraResources:");
	});

	it("records the managed payload and signing policy as release-only evidence", () => {
		expect(release_policy).toContain("Desktop integration dependency gates");
		expect(release_policy).toContain("emits only `.dist/electron-release/win-unpacked`");
		expect(release_policy).toContain("not emit NSIS");
		expect(release_policy).toContain("Electron Builder's standard Windows signing path");
		expect(release_policy).toContain("requires status `Valid`");
		expect(release_policy).toContain("required release-only evidence");
	});

	it("makes the installed ae payload and ownership boundary mandatory", () => {
		const verifier = readFileSync(
			resolve(workspace_root, ".tests/deep/desktop/verify-packaged-desktop.ps1"),
			"utf8",
		);
		const workflow = readFileSync(
			resolve(workspace_root, ".github/workflows/release-validation.yml"),
			"utf8",
		);
		expect(workflow).toContain("name: Qualified product / Windows x64");
		expect(workflow).not.toContain("if: ${{ inputs.run_packaged_desktop }}");
		expect(verifier).toContain("$embedded_forge");
		expect(verifier).toContain("must not embed a parallel Forge lifecycle");
		expect(verifier).toContain("Packaged desktop renderer evidence");
		/** The verifier now proves the honest renderer shape, not a launcher-only ASAR. */
		expect(verifier).toContain('"/frontend/index.html"');
		expect(verifier).toContain("loopback Forge CSP allowance");
		expect(verifier).toContain('"/preload.cjs"');
		expect(verifier).not.toContain("ARTISAN_PACKAGED_SMOKE");
		expect(verifier).not.toContain("Stop-Process");
		expect(workflow).toContain('ARTISAN_ALLOW_UNSIGNED_PRERELEASE: "1"');
		expect(workflow).toContain(
			"ARTISAN_RELEASE_SIGNING_KEY_PEM: ${{ secrets.ARTISAN_RELEASE_SIGNING_KEY_PEM }}",
		);
		expect(workflow).toContain("ARTISAN_RELEASE_CHANNEL: beta");
		expect(workflow).not.toContain("ARTISAN_WINDOWS_CSC_LINK");
	});

	it("parses the packaged verifier before release execution on Windows", () => {
		if (process.platform !== "win32") return;
		const verifier_path = resolve(
			workspace_root,
			".tests/deep/desktop/verify-packaged-desktop.ps1",
		);
		const parsed = spawnSync(
			"powershell",
			[
				"-NoProfile",
				"-Command",
				"$tokens=$null; $errors=$null; [System.Management.Automation.Language.Parser]::ParseFile($env:ARTISAN_PS_PARSE_TARGET,[ref]$tokens,[ref]$errors) | Out-Null; if ($errors.Count -ne 0) { throw ($errors | Out-String) }",
			],
			{
				encoding: "utf8",
				env: { ...process.env, ARTISAN_PS_PARSE_TARGET: verifier_path },
			},
		);

		expect(parsed.status, `${parsed.stdout}\n${parsed.stderr}`).toBe(0);
	});
});
