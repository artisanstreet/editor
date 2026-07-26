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

	it("runs the packaged smoke through the Forge HTTP and WebSocket daemon", () => {
		const main = readFileSync(resolve(workspace_root, "modules/desktop/src/main.ts"), "utf8");
		const supervisor = readFileSync(
			resolve(workspace_root, "modules/desktop/src/forge-process-supervisor.ts"),
			"utf8",
		);

		expect(has_electron_packaging_configuration()).toBe(true);
		expect(main).toContain("ARTISAN_PACKAGED_SMOKE");
		expect(main).toContain("ARTISAN_PACKAGED_SMOKE_USER_DATA");
		expect(main).toContain("window.loadURL(forge_http_endpoint.toString())");
		expect(main).toContain("forge_websocket_endpoint");
		expect(main).toContain("has_native_bridge");
		expect(supervisor).toContain("paths.forge_executable_path");
		expect(supervisor).toContain("ARTISAN_STATIC_FRONTEND_ROOT");
		expect(supervisor).toContain("ARTISAN_AUTH_TOKEN");
		expect(supervisor).toContain("/api/pair/request");
		expect(supervisor).not.toContain("searchParams.set");
		expect(supervisor).not.toContain("MessagePort");
		expect(desktop_config).toContain("koffi-win32-x64");
		expect(desktop_config).toContain("ssr: { noExternal: true }");
		expect(forge_config).toContain('"ae.cmd"');
		expect(forge_config).toContain("ARTISAN_NATIVE_RUNTIME=%~dp0native-runtime");
		expect(forge_config).toContain('"update-user-path.ps1"');
		const builder = readFileSync(resolve(workspace_root, "desktop-builder.yml"), "utf8");
		expect(builder).toContain("include: .scripts/package/nsis/artisan-path.nsh");
		const path_installer = readFileSync(
			resolve(workspace_root, ".scripts/package/nsis/artisan-path.nsh"),
			"utf8",
		);
		expect(path_installer).toContain("-Action Add");
		expect(path_installer).toContain("-Action Remove");
	});

	it("records the packaged layout and native unpack policy as release-only evidence", () => {
		expect(release_policy).toContain("Desktop integration dependency gates");
		expect(release_policy).toContain("expected package layout");
		expect(release_policy).toContain("`node-pty` and Koffi are explicitly staged and unpacked");
		expect(release_policy).toContain("trusted native mouse input");
		expect(release_policy).toContain("required release-only evidence");
	});

	it("makes the separate Forge process and connected renderer mandatory", () => {
		const verifier = readFileSync(
			resolve(workspace_root, ".tests/deep/desktop/verify-packaged-desktop.ps1"),
			"utf8",
		);
		const workflow = readFileSync(
			resolve(workspace_root, ".github/workflows/release-validation.yml"),
			"utf8",
		);
		expect(workflow).toContain("name: Required packaged desktop release gate");
		expect(workflow).not.toContain("if: ${{ inputs.run_packaged_desktop }}");
		expect(verifier).toContain("$artifact_forge_executable");
		expect(verifier).toContain("$record.forge_pid -eq $process.Id");
		expect(verifier).toContain('StartsWith("ws://127.0.0.1:")');
		expect(verifier).toContain("$record.renderer.has_native_bridge");
		expect(verifier).toContain("Artisan Forge survived desktop smoke shutdown");
		expect(verifier).toContain("Copy-Item -LiteralPath $artifact_release_root");
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
