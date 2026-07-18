import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../../..");
const frontend_root = resolve(workspace_root, "modules/frontend");
const frontend_config = readFileSync(resolve(frontend_root, "vite.config.ts"), "utf8");
const frontend_manifest = JSON.parse(
	readFileSync(resolve(frontend_root, "package.json"), "utf8"),
) as {
	readonly dependencies: Readonly<Record<string, string>>;
	readonly devDependencies: Readonly<Record<string, string>>;
};
const release_policy = readFileSync(
	resolve(workspace_root, "docs/release/validation-policy.md"),
	"utf8",
);

const known_electron_package_files = [
	"electron-builder.yml",
	"electron-builder.yaml",
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

function has_mounted_browser_runner() {
	const dependencies = {
		...frontend_manifest.dependencies,
		...frontend_manifest.devDependencies,
	};

	return (
		Object.hasOwn(dependencies, "@playwright/test") ||
		Object.hasOwn(dependencies, "vitest-browser-playwright")
	);
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

	it.skipIf(!has_electron_packaging_configuration())(
		"validates packaged layout and native unpack policy once the Electron bootstrap lands",
		() => {
			expect(release_policy).toContain("Desktop integration dependency gates");
			expect(release_policy).toContain("expected package layout");
			expect(release_policy).toContain("native addon is explicitly unpacked");
		},
	);

	it.skipIf(!has_mounted_browser_runner())(
		"runs mounted accessibility and responsive validation once a browser runner is selected",
		() => {
			expect(release_policy).toContain("mounted keyboard/focus");
			expect(release_policy).toContain("computed responsive-layout");
			expect(release_policy).toContain("real browser-zoom checks");
		},
	);
});
