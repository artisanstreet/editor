import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const settings_directory = resolve("modules/frontend/src/routes/components/settings");
const retained_consumers = [
	"agent-names.svelte",
	"nav.svelte",
	"models.svelte",
	"compaction-model.svelte",
	"usage-recovery.svelte",
];

describe("settings retained defaults loading", () => {
	it("mounts from retained defaults and follows controller changes without rehydrating Forge", () => {
		for (const name of retained_consumers) {
			const source = readFileSync(resolve(settings_directory, name), "utf8");

			expect(source, name).toContain("yield* SessionDefaultsController");
			expect(source, name).toContain("yield* defaults_controller.Current");
			expect(source, name).toContain("defaults_controller.Changes");
			expect(source, name).not.toContain("defaults_controller.Refresh");
			expect(source, name).not.toContain("GetRuntimeCatalog");
			expect(source, name).not.toContain("GetSessionDefaults");
			expect(source, name).not.toContain("GetModelFavorites");
			expect(source, name).not.toContain("ArtisanClient");
		}
	});

	it("leaves failed mutations to the controller's retained reconciliation", () => {
		const usage_recovery = readFileSync(
			resolve(settings_directory, "usage-recovery.svelte"),
			"utf8",
		);
		const agent_names = readFileSync(resolve(settings_directory, "agent-names.svelte"), "utf8");
		const selector = readFileSync(
			resolve(settings_directory, "../model-selector/view.svelte"),
			"utf8",
		);

		for (const [name, source] of [
			["usage recovery", usage_recovery],
			["agent names", agent_names],
			["model selector", selector],
		] as const) {
			expect(source, name).not.toContain("defaults_controller.Refresh");
			expect(source, name).not.toContain("controller.Refresh");
		}
		expect(selector).toContain("Effect.catch(() => Effect.void)");
	});
});
