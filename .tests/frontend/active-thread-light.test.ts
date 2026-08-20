import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("active thread lamp", () => {
	it("places one active lamp in each thread-list coordinate space", () => {
		const rail = Read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		expect(rail).toContain('import ActiveThreadLight from "./active-thread-light.svelte"');
		expect(rail.match(/<ActiveThreadLight/g) ?? []).toHaveLength(2);
		expect(rail).toContain("bind:this={working_light_surface}");
		expect(rail).toContain("bind:this={recent_light_surface}");
		expect(rail.match(/aria-current=\{is_active \? "page" : undefined\}/g) ?? []).toHaveLength(
			2,
		);
		expect(rail).toContain("layout_key={working_light_layout}");
		expect(rail).toContain("layout_key={recent_light_layout}");
		expect(rail).toContain("const active_thread_id = $derived(open_thread?.thread_id);");
		expect(rail.match(/thread\.thread_id === active_thread_id/g) ?? []).toHaveLength(2);
		expect(rail).not.toContain("ThreadRouteId(thread.thread_id) === active_route_id");
	});

	it("turns the provider lamp sideways and preserves its motion contract", () => {
		const lamp = Read("modules/frontend/src/routes/components/active-thread-light.svelte");
		const engine_picker = Read(
			"modules/frontend/src/routes/components/model-selector/engine-section.svelte",
		);

		expect(lamp).toContain('yield* Effect.sleep("1 millis")');
		expect(lamp).toContain("yield* RunBrowserDom(() =>");
		expect(lamp).toContain("current_surface.scrollTop");
		expect(lamp).toContain("row_rect.top - surface_rect.top + scroll_top");
		expect(lamp).toContain(
			"indicator_animated = indicator_visible && lit_thread_id !== thread_id;",
		);
		expect(lamp).toContain("data-active={indicator_visible}");
		expect(lamp).toContain("data-animate={indicator_animated}");
		expect(lamp).toContain("to right,");
		expect(lamp).toContain("ellipse 70% 48% at 35% 50%");
		for (const stop of ["32%) 0%", "10%) 26%", "2%) 52%", "transparent 74%"])
			expect(lamp).toContain(stop);
		expect(engine_picker).toContain('class="engine-light"');
		expect(lamp).toContain("transform var(--duration-fast) var(--ease-smooth-out)");
		expect(lamp).toContain("height var(--duration-fast) var(--ease-smooth-out)");
		expect(lamp).toContain("@media (prefers-reduced-motion: reduce)");
		expect(lamp).toContain("transition: none !important");
		expect(lamp).not.toMatch(/runSync|runPromise|runFork|ManagedRuntime/u);
	});
});
