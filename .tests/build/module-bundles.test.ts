import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	bundled_modules,
	entry_output_path,
	module_exports,
	repository_root,
} from "../../.scripts/build/modules.ts";

const manifest_of = (directory: string) =>
	JSON.parse(readFileSync(resolve(repository_root, directory, "package.json"), "utf8")) as {
		readonly exports: Record<string, unknown>;
		readonly name: string;
	};

describe("bundled workspace modules", () => {
	it("declares a manifest entry for every module it claims to bundle", () => {
		for (const module of bundled_modules) {
			const manifest = manifest_of(module.directory);

			expect(manifest.name).toBe(module.name);
			for (const file of Object.values(module.entries)) {
				expect(existsSync(resolve(repository_root, module.directory, file))).toBe(true);
			}
			for (const file of Object.values(module.internal_entries ?? {})) {
				expect(existsSync(resolve(repository_root, module.directory, file))).toBe(true);
			}
		}
	});

	it("keeps every package.json exports map generated from the manifest", () => {
		for (const module of bundled_modules) {
			expect(manifest_of(module.directory).exports).toEqual(module_exports(module));
		}
	});

	it("resolves editor modules to their bundle and keeps source for type-checking", () => {
		for (const module of bundled_modules.filter((candidate) => candidate.ships_in_editor)) {
			const exports_map = manifest_of(module.directory).exports;

			for (const [subpath, file] of Object.entries(module.entries)) {
				expect(exports_map[subpath]).toEqual({
					default: entry_output_path(subpath),
					development: `./${file}`,
					types: `./${file}`,
				});
			}
		}
	});

	/**
	 * The dashboard packages are loaded by the plain Node scripts in `.scripts`,
	 * including the one that produces the bundles. Pointing them at `.dist`
	 * would make the build depend on its own output.
	 */
	it("keeps the terminal dashboards resolving to source", () => {
		const tooling = bundled_modules.filter((candidate) => !candidate.ships_in_editor);

		expect(tooling.map((module) => module.name)).toEqual([
			"@artisan/dev-tui",
			"@artisanstreet/checklist",
			"@artisanstreet/runner",
		]);

		for (const module of tooling) {
			const exports_map = manifest_of(module.directory).exports;

			for (const [subpath, file] of Object.entries(module.entries)) {
				expect(exports_map[subpath]).toBe(`./${file}`);
			}
		}
	});

	it("leaves the modules that own their build out of the bundle set", () => {
		const bundled = new Set(bundled_modules.map((module) => module.directory));

		/** Vite already builds these two, and `data` is a JSON asset tree. */
		expect(bundled.has("modules/frontend")).toBe(false);
		expect(bundled.has("modules/installer")).toBe(false);
		expect(bundled.has("modules/data")).toBe(false);
	});

	it("is wired into the release loop and reachable on its own", () => {
		const manifest = JSON.parse(
			readFileSync(resolve(repository_root, "package.json"), "utf8"),
		) as { readonly scripts: Record<string, string> };

		expect(manifest.scripts["build:modules"]).toBe("node .scripts/build/build-modules.ts");
		expect(
			readFileSync(resolve(repository_root, ".scripts/build/runner.ts"), "utf8"),
		).toContain("module_build_steps()");
	});
});
