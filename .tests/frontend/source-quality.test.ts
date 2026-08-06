import { readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const selector_root = resolve("modules/frontend/src/routes/components");
const selector_directory = resolve(selector_root, "model-selector");
const selector_sources = [
	resolve(selector_root, "model-selector", "view.svelte"),
	...readdirSync(selector_directory)
		.filter((name) => name.endsWith(".svelte") || name.endsWith(".ts"))
		.map((name) => resolve(selector_directory, name)),
	resolve("modules/frontend/src/lib/settings/session-defaults-controller.ts"),
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

	it("keeps selector mutation and lifecycle work in focused generator controllers", () => {
		const root = readFileSync(resolve(selector_root, "model-selector", "view.svelte"), "utf8");
		const policy_controller = readFileSync(
			resolve(selector_directory, "policy-controller.ts"),
			"utf8",
		);
		const defaults_controller = readFileSync(
			resolve("modules/frontend/src/lib/settings/session-defaults-controller.ts"),
			"utf8",
		);

		expect(root).toContain("const FlushPolicy = Effect.gen");
		expect(root).toContain("policy_controller.Flush(PersistPolicy)");
		expect(policy_controller).toContain("export const MakeModelPolicyController = Effect.gen");
		expect(policy_controller).toContain(
			"const FlushUnlocked = (persist: PolicyPersistence) =>",
		);
		expect(policy_controller).toContain("while (true)");
		expect(defaults_controller).toContain(
			"export const SessionDefaultsControllerLive = Layer.effect",
		);
		expect(defaults_controller).toContain("const SaveCompactionDefaults = (");
		expect(defaults_controller).toContain("const SetFavorite = (");
		expect(root).toContain("<EngineSection");
		expect(root).toContain("<ModelList");
		expect(root).toContain("<PolicyControls");

		const engine_section = readFileSync(
			resolve(selector_directory, "engine-section.svelte"),
			"utf8",
		);
		expect(engine_section).toContain('<script lang="ts" effect>');
		expect(engine_section).toContain("yield* PositionIndicator");
		expect(engine_section).not.toMatch(/Effect\.run(?:Fork|Promise|Sync)/);
	});
});
