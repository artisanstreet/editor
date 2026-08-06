import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import type { SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";
import { ComposerContextWindowTokens } from "../../modules/frontend/src/lib/composer/send-readiness";
import { OfflineRuntimeCatalog } from "../../modules/frontend/src/lib/runtime/offline-catalog";

const read = (path: string) => readFileSync(resolve(path), "utf8");

const policy = (context_window?: string): ThreadSessionPolicy => ({
	engine_id: "claude",
	model: "claude-fable-5",
	...(context_window === undefined ? {} : { context_window }),
	permission: "supervised",
	permission_mode: "on_request",
	reasoning_effort: "high",
	sandbox_mode: "workspace_write",
	service_tier: "standard",
	strict_clarification: false,
	web_search_enabled: false,
});

describe("context window denominator", () => {
	/**
	 * The launcher composes the model id as `model + (suffix ?? "")`, and a bare
	 * Claude 5 id resolves to the harness default — the extended window. Live
	 * sessions on suffix-less ids measured 236K+ tokens of context; dividing
	 * those readings by the 200K standard option pinned the gauge at 100%.
	 */
	it("resolves an unset policy choice to the capability default, not the base window", () => {
		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), undefined)).toBe(
			1_000_000,
		);
	});

	it("resolves an explicit suffix to its own option", () => {
		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy("[1m]"), undefined)).toBe(
			1_000_000,
		);
	});

	it("falls back to the capability default for a suffix the catalog does not know", () => {
		expect(
			ComposerContextWindowTokens(OfflineRuntimeCatalog, policy("[legacy]"), undefined),
		).toBe(1_000_000);
	});

	/** A provider that states its usable window on the wire outranks the catalog. */
	it("prefers the provider-reported window over the catalog option", () => {
		const reported: SurfaceUsageAggregate = {
			context_tokens: 128_000,
			context_window_tokens: 258_400,
			scope: "run",
			scope_id: "run_1",
		};

		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), reported)).toBe(
			258_400,
		);
	});

	it("shows no gauge for a model without the capability", () => {
		expect(
			ComposerContextWindowTokens(
				OfflineRuntimeCatalog,
				{ ...policy(), model: "unknown-model" },
				undefined,
			),
		).toBeUndefined();
	});
});

describe("context window card", () => {
	/**
	 * The card states the reading and restates it as one bar. The per-turn
	 * dither chart it replaced drew history the hover could not usefully carry,
	 * and its per-turn points were the only reason the card needed a
	 * pointer-holdable surface and a series fetch of its own.
	 */
	it("states the window per model and shows one progress bar, not a chart", () => {
		const details = read("modules/frontend/src/routes/components/context-usage-details.svelte");

		expect(details).toContain(">Context Window<");
		expect(details).toContain("The context window for");
		expect(details).toContain("has a context window of");
		expect(details).toContain('from "$lib/components/ui/progress"');
		/** The bar sits at the bottom, after the prose block. */
		expect(details.indexOf("has a context window of")).toBeLessThan(
			details.indexOf("<Progress"),
		);
		expect(details).toContain("<Progress");
		expect(details).not.toContain("DitherStackedArea");
		expect(details).not.toContain("Context used, per turn");
		/** Neither harness reports a by-category breakdown; the card must not invent one. */
		expect(details).not.toContain("Cached input");
	});

	/** The reading is clamped: an over-full report renders a full bar, not a broken one. */
	it("clamps the bar's value into the renderable range", () => {
		const details = read("modules/frontend/src/routes/components/context-usage-details.svelte");

		expect(details).toContain("Math.min(100, Math.max(0, percent))");
	});

	it("no longer fetches the per-turn series for the hover", () => {
		const gauge = read("modules/frontend/src/routes/components/context-usage-gauge.svelte");

		expect(gauge).not.toContain("GetThreadUsageSeries");
		expect(gauge).not.toContain("series");
	});

	/**
	 * The backend series stays scoped to one context window: a compaction
	 * replaces the history, so turns before it no longer occupy the window.
	 */
	it("scopes the series to one context window rather than the whole thread", () => {
		const service = read("modules/backend/src/surfaces/service.ts");

		expect(service).toContain('eq(SurfaceItems.kind, "compaction")');
		expect(service).toMatch(
			/gt\(\s*SurfaceItems\.projection_order,\s*boundary\.projection_order,?\s*\)/,
		);
	});
});
