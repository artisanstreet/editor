import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const ReadSettings = (name: string) =>
	readFileSync(resolve("modules/frontend/src/routes/components/settings", name), "utf8");

describe("settings SER remediation", () => {
	it("keeps retention reconciliation and validation in direct generator programs", () => {
		const source = ReadSettings("threads.svelte");

		expect(source).toContain("const SavePolicy = (next: ThreadRetentionPolicy) =>");
		expect(source).toContain("Effect.gen(function* ()");
		expect(source).toContain("yield* client.UpdateThreadRetentionPolicy(next)");
		expect(source).toContain("Could not save retention policy");
		expect(source).toContain('{ readonly _tag: "Unverified" }');
		expect(source).toContain("Retention policy remains unverified");
		expect(source).toContain("Controls are disabled");
		expect(source).not.toContain("const fallback = policy");
		expect(source).toContain("if (policy === undefined || saving) return");
		expect(source).toContain("Effect.ensuring(");
		expect(source).toContain("const parsed = Number(days_text)");
		expect(source).not.toContain("Number.parseInt");
		expect(source).not.toContain("Queue.offerUnsafe");
		expect(source).not.toContain("Effect.sync");
	});

	it("uses one direct defaults write for each compaction action", () => {
		const source = ReadSettings("compaction-model.svelte");

		expect(source).toContain("const SaveDefaults =");
		expect(source).toContain("yield* defaults_controller.SaveCompactionDefaults");
		expect(source).toContain('{ _tag: "Explicit", model_id: previewed_model.id }');
		expect(source).toContain("const UpdateThinking =");
		expect(source).toContain("const UpdateContext =");
		expect(source).toContain("const UpdatePermission =");
		expect(source).toContain(
			"onfocusin={yield* HoverMode(input.mode, move_hover, event)}",
		);
		expect(source).not.toContain("Queue.offerUnsafe");
		expect(source).not.toContain("PolicyControls");
		expect(source).not.toContain("SpeedOption");
	});

	it("derives settings catalog, availability, favorites, and defaults from controller changes", () => {
		const compaction = ReadSettings("compaction-model.svelte");
		const engine = ReadSettings("engine.svelte");
		const models = ReadSettings("models.svelte");
		const nav = ReadSettings("nav.svelte");

		for (const source of [compaction, engine, models, nav]) {
			expect(source).toContain("yield* SessionDefaultsController");
			expect(source).toContain("defaults_controller.Changes");
			expect(source).toContain("$derived(defaults_state.catalog)");
			expect(source).not.toContain("GetRuntimeCatalog");
			expect(source).not.toContain("RuntimeCatalogChanges");
		}
		expect(compaction).toContain("$derived(defaults_state.available)");
		expect(compaction).toContain("$derived(defaults_state.defaults)");
		expect(models).toContain("defaults_state.favorite_ids");
		expect(models).toContain("$derived(defaults_state.available)");
	});

	it("removes settings command-bus queues and mount-time redirect control flow", () => {
		const models = ReadSettings("models.svelte");
		const engine = ReadSettings("engine.svelte");
		const entry = readFileSync(
			resolve("modules/frontend/src/routes/settings/+page.svelte"),
			"utf8",
		);

		expect(models).not.toContain("Queue.offerUnsafe");
		expect(engine).not.toContain("Queue.offerUnsafe");
		expect(engine).toContain("const RefreshUsage = Effect.gen(function* ()");
		expect(entry).not.toContain("onMount");
		expect(entry).not.toContain("goto(");
		expect(entry).toContain("<SettingsModels />");
	});
});
