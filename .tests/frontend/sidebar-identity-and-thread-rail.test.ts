import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { weekly_reset_duration } from "../../modules/frontend/src/lib/identity/weekly-reset";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar identity and thread rail regressions", () => {
	it("fans provider usage reads out through Effect concurrency without a component queue", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const refresh_controller = read(
			"modules/frontend/src/lib/identity/usage-refresh-controller.ts",
		);

		expect(Effect.forkScoped).toBeTypeOf("function");
		expect(identity).toContain("refresh_controller.Refresh(");
		expect(refresh_controller).toContain("Effect.forEach(");
		expect(refresh_controller).toContain('{ concurrency: "unbounded", discard: true }');
		expect(refresh_controller).not.toContain("Queue.unbounded");
		expect(refresh_controller).not.toMatch(/\bEffect\.fork\(/);
	});

	/**
	 * The enabled engine set is known from the catalog before any provider
	 * answers, so the menu paints one named row per enabled engine from the
	 * first frame — skeleton meters until that engine's first report lands —
	 * in stable catalog order, instead of an anonymous skeleton that later
	 * re-resolves into named sections in provider-answer order. The fan-out
	 * itself starts at mount, so an open usually shows readings straight away.
	 */
	it("renders every enabled engine as a named row from the first frame", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(identity).toContain("The fan-out starts at mount, not at first open");
		expect(identity).toContain("engine_ids={usage_engine_ids}");
		expect(usage).toContain("const engine_rows = $derived(");
		/** Catalog order keys the rows; answer order never reshuffles them. */
		expect(usage).toContain("{#each engine_rows as row, row_index (row.engine_id)}");
		expect(usage).toContain('{:else if row.kind === "pending"}');
		expect(usage).toContain("usage loading");
		/** The identity is real even while the reading is pending. */
		expect(usage).toContain("{EngineDisplayName(row.engine_id)}");
		/** A menu with only pending rows must not claim nothing is connected. */
		expect(usage).toContain("engine_rows.length === 0");
		expect(usage).toContain('{#if usage_state.status === "loaded"}');
	});

	it("keeps unavailable provider errors out of the provider header", () => {
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");
		const unavailable_row = usage.slice(usage.lastIndexOf("{:else}"));

		expect(unavailable_row).toContain(
			'<span class="truncate text-xs font-medium text-foreground">{engine.display_name}</span>',
		);
		expect(unavailable_row).toContain(
			'{engine.failure ?? "Usage is unavailable right now."}',
		);
		expect(unavailable_row).not.toContain("— usage unavailable");
	});

	it("shows the latest trustworthy weekly reset on each shader-glass provider menu", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");
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
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(motion).toContain('import { RunBrowserDom } from "$lib/browser/dom"');
		expect(motion).toContain("export const MotionEasing = () =>");
		expect(motion).toContain("export const MotionDuration = () =>");
		expect(motion).toContain("yield* RunBrowserDom(() =>");
		expect(usage).toContain("const motion_duration = yield* MotionDuration();");
		expect(usage).toContain("const motion_easing = yield* MotionEasing();");
	});

	it("keeps thread-list rows stationary while retaining proximity reveal", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");
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
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

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
		const layout = read("modules/frontend/src/routes/+layout.svelte");
		const panel = read("modules/frontend/src/routes/components/sectioned-panel.svelte");

		expect(layout).toContain("const is_thread_route = $derived(");
		expect(layout).toContain("show_thread_hover_rail={is_thread_route}");
		expect(panel).toContain("show_thread_hover_rail: boolean;");
		expect(panel).toContain("{#if show_thread_hover_rail && threads.length > 0}");
		expect(panel).not.toContain('{#if surface === "threads" && threads.length > 0}');
	});
});
