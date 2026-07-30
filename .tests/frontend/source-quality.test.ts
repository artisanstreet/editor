import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const selector_root = resolve("modules/frontend/src/routes/components");
const selector_directory = resolve(selector_root, "model-selector");
const selector_sources = [
	resolve(selector_root, "model-selector", "view.sv"),
	...readdirSync(selector_directory)
		.filter((name) => name.endsWith(".sv"))
		.map((name) => resolve(selector_directory, name)),
];

describe("frontend source quality", () => {
	it("keeps the model selector composition and focused views below 600 lines", () => {
		for (const path of selector_sources) {
			const source = readFileSync(path, "utf8");
			const physical_lines = source.split(/\r?\n/).length;

			expect(physical_lines, path).toBeLessThan(600);
		}
	});

	it("keeps ad hoc async lifecycle machinery out of selector views", () => {
		const forbidden = [
			/\bnew Promise\b/,
			/\bfetch\s*\(/,
			/\bset(?:Timeout|Interval)\s*\(/,
			/\b(?:request|cancel)AnimationFrame\s*\(/,
			/\.addEventListener\s*\(/,
			/\bEffect\.runFork\s*\(/,
		];

		for (const path of selector_sources) {
			const source = readFileSync(path, "utf8");
			for (const pattern of forbidden) {
				expect(source, `${path} contains ${pattern}`).not.toMatch(pattern);
			}
		}
	});

	it("keeps scoped effects in the root and view ownership in focused components", () => {
		const root = readFileSync(resolve(selector_root, "model-selector", "view.sv"), "utf8");

		expect(root).toContain("Effect.forkScoped");
		expect(root).toContain("<EngineSection");
		expect(root).toContain("<ModelList");
		expect(root).toContain("<PolicyControls");
		expect(root).toContain("<CompactionControl");

		const engine_section = readFileSync(
			resolve(selector_directory, "engine-section.sv"),
			"utf8",
		);
		expect(engine_section).toContain('<script lang="ts" effect>');
		expect(engine_section).toContain("Effect.forkScoped");
	});
});
