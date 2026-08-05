import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { weekly_reset_duration } from "../../modules/frontend/src/lib/identity/weekly-reset";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar identity and thread rail regressions", () => {
	it("fans provider usage reads out through Effect concurrency without a component queue", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");

		expect(Effect.forkScoped).toBeTypeOf("function");
		expect(identity).toContain("Effect.forEach(");
		expect(identity).toContain('{ concurrency: "unbounded", discard: true }');
		expect(identity).toContain("FetchEngineUsage(engine_id, force)");
		expect(identity).not.toContain("Queue.unbounded");
		expect(identity).not.toMatch(/\bEffect\.fork\(/);
	});

	/**
	 * Reports merge in as each provider answers, so a slow provider was simply
	 * absent until its first report landed — Claude read as not picked up while
	 * Codex painted instantly. An engine still fetching its first report shows
	 * its real mark and name over skeleton meters instead of nothing.
	 */
	it("names an engine awaiting its first usage report over a skeleton", () => {
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.sv");

		expect(usage).toContain("const pending_engines = $derived(");
		expect(usage).toContain("{#each pending_engines as engine_id, pending_index (engine_id)}");
		expect(usage).toContain("usage loading");
		/** The identity is real even while the reading is pending. */
		expect(usage).toContain("{EngineDisplayName(engine_id)}");
		/** A menu with only pending engines must not claim nothing is connected. */
		expect(usage).toContain(
			"authenticated_engines.length === 0 && unavailable_engines.length === 0 && pending_engines.length === 0",
		);
	});

	it("shows the latest trustworthy weekly reset on each shader-glass provider menu", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.sv");
		const now = Date.parse("2026-07-31T12:00:00.000Z");

		expect(
			weekly_reset_duration(
				[
					{
						id: "session",
						kind: "session",
						percent_used: 10,
						resets_at: "2026-08-07T12:00:00.000Z",
					},
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{
						id: "model-weekly",
						kind: "weekly",
						percent_used: 30,
						resets_at: "2026-08-02T12:00:00.000Z",
					},
				],
				now,
			),
		).toBe("2 days");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T17:00:00.000Z",
					},
				],
				now,
			),
		).toBe("5 hours");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T12:45:00.000Z",
					},
				],
				now,
			),
		).toBe("45 minutes");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{ id: "model-weekly", kind: "weekly", percent_used: 30 },
				],
				now,
			),
		).toBeUndefined();
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T11:00:00.000Z",
					},
				],
				now,
			),
		).toBeUndefined();

		expect(usage).toContain("weekly_reset_duration(engine.windows, checked_at_ms)");
		expect(identity).toContain('<ShaderGlassSurface class="w-full rounded-2xl">');
		expect(identity).toContain("bg-transparent! p-0! shadow-none! ring-0!");
		expect(usage).toContain('<div class="flex flex-col px-1 py-1">');
		expect(usage).not.toContain("flex flex-col gap-2.5 px-1 py-1");
		expect(usage).toContain('<DropdownMenuSeparator class="my-1" />');
		expect(usage).toContain(
			'Your weekly limit resets in <span class="text-foreground">{weekly_reset}</span>.',
		);
	});

	it("yields sidebar motion token reads through the browser DOM boundary", () => {
		const motion = read("modules/frontend/src/lib/identity/usage-window-motion.ts");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.sv");

		expect(motion).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(motion).toContain("export const MotionEasing = () =>");
		expect(motion).toContain("export const MotionDuration = () =>");
		expect(motion).toContain("yield* RunBrowserDom(() =>");
		expect(usage).toContain("const motion_duration = yield* MotionDuration();");
		expect(usage).toContain("const motion_easing = yield* MotionEasing();");
	});

	it("keeps thread-list rows stationary while retaining proximity reveal", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.sv");
		const styles = read("modules/frontend/src/lib/styles/sidebar.css");

		expect(rail).toContain("<svelte:window onpointermove={yield* TrackPointer(event)} />");
		expect(rail).toContain('class="t-panel-slide-x');
		expect(rail).not.toContain("SetShifts");
		expect(rail).not.toContain("getComputedStyle");
		expect(rail).not.toContain("t-avatar");
		expect(styles).not.toContain("--avatar-");
		expect(styles).not.toContain(".t-avatar");
	});

	it("stands the proximity rail down while the account menu covers its band", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.sv");
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.sv");

		expect(identity).toContain("open = $bindable(false)");
		expect(identity).not.toContain("let open = $state(false);");
		expect(panel).toContain("<SidebarIdentity bind:open={account_open} />");
		expect(panel).toContain(
			"<ThreadHoverRail suppressed={account_open || inspecting_image} {threads} />",
		);
		expect(rail).toContain("suppressed = false,");
		expect(rail).toContain("const open = $derived(near && !suppressed);");
		// Suppression drops proximity outright, so closing the menu cannot fire a banked reveal.
		expect(rail).toMatch(/if \(suppressed\) \{\s*yield\* Conceal\(\);\s*return;\s*\}/);
		expect(rail).toContain("if (suppressed) return;");
	});

	it("mounts the thread hover rail only on canonical conversation routes", () => {
		const layout = read("modules/frontend/src/routes/+layout.sv");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(layout).toContain("const is_thread_route = $derived(");
		expect(layout).toContain("show_thread_hover_rail={is_thread_route}");
		expect(panel).toContain("show_thread_hover_rail: boolean;");
		expect(panel).toContain("{#if show_thread_hover_rail && threads.length > 0}");
		expect(panel).not.toContain('{#if surface === "threads" && threads.length > 0}');
	});
});
