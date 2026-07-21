import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../../..");
const frontend_root = resolve(workspace_root, "modules/frontend");
const frontend_config = readFileSync(resolve(frontend_root, "vite.config.ts"), "utf8");
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
		expect(frontend_config).not.toMatch(/\b(?:electron|node:|@artisan\/backend)\b/);
	});

	it("keeps typed Electron-port adapters in the renderer-safe transport entry", () => {
		const transport_entry = readFileSync(
			resolve(workspace_root, "modules/transport/src/index.ts"),
			"utf8",
		);
		const electron_adapter = readFileSync(
			resolve(workspace_root, "modules/transport/src/electron-message-port.ts"),
			"utf8",
		);

		expect(transport_entry).toContain('export * from "./electron-message-port"');
		expect(electron_adapter).toContain("ElectronMessagePortMainShape");
		expect(electron_adapter).toContain("ElectronRendererMessagePortShape");
		expect(electron_adapter).toContain("adapt_electron_message_port_main");
		expect(electron_adapter).toContain("adapt_electron_renderer_message_port");
		expect(electron_adapter).not.toMatch(/from\s+["']electron["']/);
	});

	it("has a dedicated packaged-Electron smoke path rather than treating source strings as evidence", () => {
		const smoke = readFileSync(
			resolve(workspace_root, "modules/desktop/src/packaged-smoke.ts"),
			"utf8",
		);
		const main = readFileSync(resolve(workspace_root, "modules/desktop/src/main.ts"), "utf8");
		const utility = readFileSync(
			resolve(workspace_root, "modules/desktop/src/utility.ts"),
			"utf8",
		);

		expect(has_electron_packaging_configuration()).toBe(true);
		expect(smoke).toContain("ArtisanClient");
		expect(smoke).toContain("ForceRestartForSmoke");
		expect(smoke).toContain("FocusAndKeyboardCreateThread");
		expect(smoke).toContain("mounted_ui");
		expect(smoke).toContain('Effect.timeout("45 seconds")');
		expect(smoke).toContain('type: "thread.create"');
		expect(main).toContain("ARTISAN_PACKAGED_SMOKE");
		expect(main).toContain("ARTISAN_PACKAGED_SMOKE_USER_DATA");
		expect(utility).toContain("verify_packaged_native_runtime");
		expect(utility).toContain("mkdirSync(dirname(environment.database_path)");
		expect(utility).toContain("bounded_file_store_native.win32-x64-msvc.node");
		expect(utility).toContain("koffi_native_binding_path");
		expect(readFileSync(resolve(workspace_root, "desktop.vite.config.ts"), "utf8")).toContain(
			"koffi-win32-x64",
		);
	});

	it("records the packaged layout and native unpack policy as release-only evidence", () => {
		expect(release_policy).toContain("Desktop integration dependency gates");
		expect(release_policy).toContain("expected package layout");
		expect(release_policy).toContain(
			"`node-pty`, the bounded native addon, and Koffi are explicitly staged",
		);
		expect(release_policy).toContain("trusted native mouse input");
		expect(release_policy).toContain("required release-only evidence");
	});

	it("makes packaged Electron mounted accessibility and responsive evidence mandatory", () => {
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
		expect(release_policy).toContain("keyboard/focus");
		expect(release_policy).toContain("computed responsive-layout");
		expect(release_policy).toMatch(/real\s+browser-zoom/);
		expect(verifier).toContain('Stop-PackagedSmokeProcesses -Reason "preflight"');
		expect(verifier).toContain('Stop-PackagedSmokeProcesses -Reason "cleanup"');
		expect(verifier).toContain("is_trusted");
		expect(verifier).toContain("active_before_click");
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
