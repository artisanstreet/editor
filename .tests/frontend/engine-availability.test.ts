import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(resolve(path), "utf8");

/**
 * The blanket availability switch. Off means the engine is not represented as
 * available anywhere — no selector section, no usage read, no models — rather
 * than shown greyed out. One persisted set drives every surface, so the
 * selector and the account menu cannot disagree about whether an engine exists.
 */
describe("engine availability", () => {
	it("persists the switch through session defaults as a disabled set", () => {
		const protocol = read("modules/protocol/src/session-defaults.ts");
		const service = read("modules/backend/src/settings/session-defaults-service.ts");
		const controller = read("modules/frontend/src/lib/settings/session-defaults-controller.ts");

		expect(protocol).toContain("disabled_engines: Schema.optional(");
		expect(protocol).toContain("enabled: Schema.Boolean");
		/** Row-per-engine: switching one engine never rewrites the others. */
		expect(service).toContain(".delete(DisabledEngines)");
		expect(service).toContain(".insert(DisabledEngines)");
		expect(controller).toContain("SetEngineEnabled");
	});

	it("leads each engine page with the switch and hides the rest while off", () => {
		const page = read("modules/frontend/src/routes/components/settings/engine.sv");

		expect(page).toContain("const engine_enabled = $derived(");
		expect(page).toContain("ToggleAvailability(!engine_enabled)");
		expect(page).toContain("{#if engine_enabled}");
		/** Off is absence, not a dimmed account card. */
		expect(page).toContain("switched off. Its models are hidden everywhere");
		/** The catalog-copy permissions list carried no controls; it is gone. */
		expect(page).not.toContain('aria-labelledby="permissions"');
		/** Flipping the switch on refetches, painting skeletons while it loads. */
		expect(page).toContain("LoadInitialUsage(engine_id, engine_enabled)");
	});

	it("removes a disabled engine from the selector and the usage fan-out", () => {
		const selector = read("modules/frontend/src/routes/components/model-selector/view.sv");
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.sv");

		expect(selector).toContain("!disabled_engines.has(harness.id)");
		expect(selector).toContain("!disabled_engines.has(model.engine)");
		expect(identity).toContain("!disabled_engine_ids.includes(engine_id)");
		/** A cached snapshot must not resurrect a switched-off engine's report. */
		expect(usage).toContain("hidden_engine_ids");
	});
});
