import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { ContextAutoCompactionPercent } from "../../modules/frontend/src/lib/context-usage/auto-compaction";
import { ContextGaugeToneMix } from "../../modules/frontend/src/lib/context-usage/gauge-tone";

/** The engine most likely to be selected, and the earliest to compact. */
const codex = ContextAutoCompactionPercent({
	harness: "codex",
	native_model_id: "gpt-5.2-codex",
	window_tokens: 272_000,
});

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("context gauge tone ramp", () => {
	it("stays calm while the window is unremarkable", () => {
		expect(ContextGaugeToneMix(0, codex)).toEqual({ danger: 0, warn: 0 });
		expect(ContextGaugeToneMix(11, codex)).toEqual({ danger: 0, warn: 0 });
		expect(ContextGaugeToneMix(50, codex)).toEqual({ danger: 0, warn: 0 });
	});

	it("warms continuously rather than snapping at a threshold", () => {
		expect(ContextGaugeToneMix(65, codex)).toEqual({ danger: 0, warn: 50 });
		expect(ContextGaugeToneMix(80, codex)).toEqual({ danger: 0, warn: 100 });
		/** Monotonic across the leg: no step, no reversal. */
		const warns = [50, 55, 60, 65, 70, 75, 80].map((p) => ContextGaugeToneMix(p, codex).warn);
		expect(warns).toEqual([...warns].sort((left, right) => left - right));
	});

	/**
	 * Full red lands on the reading's own compaction point rather than a guessed
	 * one. Codex clamps to nine tenths of the context window, so the ramp there
	 * finishes at 90 and is already fully red by the time Claude's later point
	 * would arrive.
	 */
	it("reddens across the last leg and arrives exactly at auto-compaction", () => {
		expect(ContextGaugeToneMix(85, codex)).toEqual({ danger: 50, warn: 100 });
		expect(ContextGaugeToneMix(90, codex)).toEqual({ danger: 100, warn: 100 });
		expect(ContextGaugeToneMix(100, codex)).toEqual({ danger: 100, warn: 100 });
	});

	/**
	 * The same reading is a different tone on a different model, which is the
	 * point of anchoring per harness rather than to one blanket number: 90% full
	 * is compacting on Codex and merely close on a 200K Claude window.
	 */
	it("stretches the red leg for a model that compacts later", () => {
		const claude = ContextAutoCompactionPercent({
			harness: "claude",
			native_model_id: "claude-opus-5",
			window_tokens: 200_000,
		});

		expect(claude).toBe(100);
		expect(ContextGaugeToneMix(90, claude)).toEqual({ danger: 50, warn: 100 });
		expect(ContextGaugeToneMix(90, codex).danger).toBeGreaterThan(
			ContextGaugeToneMix(90, claude).danger,
		);
		expect(ContextGaugeToneMix(100, claude)).toEqual({ danger: 100, warn: 100 });
	});

	it("clamps rather than extrapolating past either end", () => {
		expect(ContextGaugeToneMix(-20, codex)).toEqual({ danger: 0, warn: 0 });
		expect(ContextGaugeToneMix(420, codex)).toEqual({ danger: 100, warn: 100 });
	});
});

describe("auto-compaction point", () => {
	/** `(context_window * 9) / 10`, clamped, in codex-rs/protocol/src/openai_models.rs. */
	it("puts Codex at nine tenths of whatever window the model resolves", () => {
		for (const window_tokens of [272_000, 400_000, 200_000]) {
			expect(
				ContextAutoCompactionPercent({
					harness: "codex",
					native_model_id: "gpt-5.2-codex",
					window_tokens,
				}),
			).toBe(90);
		}
	});

	/** "auto-compact ... at about 967K tokens by default" on the 1M window. */
	it("puts Sonnet 5 at its documented 967K rather than at the window", () => {
		const percent = ContextAutoCompactionPercent({
			harness: "claude",
			native_model_id: "claude-sonnet-5",
			window_tokens: 1_000_000,
		});

		expect(percent).toBeCloseTo(96.7, 1);
	});

	/**
	 * A gateway or `CLAUDE_CODE_DISABLE_1M_CONTEXT` budgets Sonnet 5 at 200K and
	 * compacts at that boundary, which the token cap produces on its own.
	 */
	it("follows Sonnet 5 down to a smaller budgeted window", () => {
		expect(
			ContextAutoCompactionPercent({
				harness: "claude",
				native_model_id: "claude-sonnet-5",
				window_tokens: 200_000,
			}),
		).toBe(100);
	});

	/** Nothing is published for these, so the window is the only honest claim. */
	it("declines to invent a point for an undocumented engine", () => {
		expect(
			ContextAutoCompactionPercent({
				harness: "cursor",
				native_model_id: "composer-1",
				window_tokens: 200_000,
			}),
		).toBe(100);
		expect(
			ContextAutoCompactionPercent({
				harness: "codex",
				native_model_id: undefined,
				window_tokens: 0,
			}),
		).toBe(100);
	});
});

describe("context gauge rendering", () => {
	it("carries a semantic tone of its own rather than the trigger's currentColor", () => {
		const ring = read("modules/frontend/src/routes/components/context-usage-ring.svelte");

		expect(ring).toContain("--gauge-warn");
		expect(ring).toContain("--gauge-danger");
		expect(ring).toContain("var(--banner-info)");
		expect(ring).toContain("var(--banner-warning)");
		expect(ring).toContain("var(--banner-error)");
		/** A grey arc on a grey track was the illegibility; neither may come back. */
		expect(ring).not.toContain('stroke="currentColor"');
		expect(ring).not.toContain('stroke="var(--muted)"');
		/** Hover must not retint the gauge, or it would report a change that never happened. */
		expect(
			read("modules/frontend/src/routes/components/model-selector/view.svelte"),
		).not.toContain("model-context-gauge");
	});

	/**
	 * A hover card rather than a tooltip. A tooltip is a non-interactive aside
	 * and dismisses when the pointer leaves its trigger, so the chart's own
	 * points — which exist to be hovered for a per-turn reading — could never be
	 * reached. The caret opt-out goes with it: this primitive draws none.
	 */
	it("wears the same glass as the account menu on a surface that can be entered", () => {
		const gauge = read("modules/frontend/src/routes/components/context-usage-gauge.svelte");
		const tooltip = read(
			"modules/frontend/src/lib/components/ui/tooltip/tooltip-content.svelte",
		);

		expect(gauge).toContain("<ShaderGlassSurface");
		expect(gauge).toContain("LinkPreview.Content");
		expect(gauge).not.toContain("TooltipContent");
		/** The primitive's own fill must be stripped, or it paints over the surface. */
		expect(gauge).toContain("bg-transparent");
		expect(gauge).toContain("p-0");
		expect(gauge).toContain("shadow-none");
		/** Every ordinary tooltip keeps its caret; this opt-out must not become the default. */
		expect(tooltip).toContain("arrow = true");
		expect(tooltip).toContain("{#if arrow}");
	});

	it("names the model on the card and claims no breakdown the wire never sent", () => {
		const details = read("modules/frontend/src/routes/components/context-usage-details.svelte");
		const controls = read("modules/frontend/src/routes/components/composer/controls.svelte");
		const selector = read("modules/frontend/src/routes/components/model-selector/view.svelte");

		expect(details).toContain(">Context Window<");
		expect(details).toContain("The context window for");
		expect(controls).toContain("model_name={context_model_name}");
		expect(controls).toContain("ContextUsageModelName(context_usage, runtime_catalog)");
		expect(controls).toContain(
			"ContextUsageAutoCompactionPercent(context_usage, context_window_tokens ?? 0)",
		);
		expect(controls).not.toContain("selected_model_name");
		expect(controls).not.toContain("policy?.engine_id");
		expect(controls).not.toContain("policy?.model");
		expect(selector).not.toContain("selected_model_name");
		/**
		 * Neither harness reports tokens per category, so the card must never grow
		 * rows it would have to invent. See the component's own note.
		 */
		expect(details).not.toMatch(/System prompt|Skills|Memory files|System tools/);
	});
});
