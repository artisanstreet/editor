import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import type { SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";
import {
	ComposerContextUsageIsCurrent,
	ComposerContextWindowTokens,
} from "../../modules/frontend/src/lib/composer/send-readiness";
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
			context_origin: {
				engine_id: "claude",
				model_id: "claude-fable-5",
				run_id: "run_1",
			},
			context_tokens: 128_000,
			context_window_tokens: 258_400,
			scope: "run",
			scope_id: "run_1",
		};

		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), reported)).toBe(
			258_400,
		);
	});

	/**
	 * The stored gauge carries the newest non-null window forward across every
	 * run in a thread with no engine scoping, so a Codex run's reported window
	 * survived onto Claude threads and the gauge read 252K beside a picker
	 * saying 1M. Telemetry belongs to the run that produced it.
	 */
	it("ignores a window reported by a different engine than the thread now targets", () => {
		const reported: SurfaceUsageAggregate = {
			context_origin: {
				engine_id: "codex",
				model_id: "gpt-5-codex",
				run_id: "run_codex",
			},
			context_tokens: 128_000,
			context_window_tokens: 252_000,
			scope: "run",
			scope_id: "run_codex",
		};

		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), reported)).toBe(
			1_000_000,
		);
		expect(ComposerContextUsageIsCurrent(policy(), reported)).toBe(false);
	});

	it("ignores a window reported by a different model on the same engine", () => {
		const reported: SurfaceUsageAggregate = {
			context_origin: {
				engine_id: "claude",
				model_id: "claude-opus-4-5",
				run_id: "run_prior",
			},
			context_tokens: 128_000,
			context_window_tokens: 252_000,
			scope: "run",
			scope_id: "run_prior",
		};

		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), reported)).toBe(
			1_000_000,
		);
	});

	/**
	 * A run whose model was never recorded still matches on its engine. Dropping
	 * the gauge for every such run is a wider silence than the mismatch the
	 * scoping guards against.
	 */
	it("accepts a same-engine reading whose model was never recorded", () => {
		const reported: SurfaceUsageAggregate = {
			context_origin: { engine_id: "claude", run_id: "run_1" },
			context_tokens: 128_000,
			context_window_tokens: 258_400,
			scope: "run",
			scope_id: "run_1",
		};

		expect(ComposerContextUsageIsCurrent(policy(), reported)).toBe(true);
		expect(ComposerContextWindowTokens(OfflineRuntimeCatalog, policy(), reported)).toBe(
			258_400,
		);
	});

	/** No origin at all is no reading; the catalog denominator stands alone. */
	it("treats an origin-less aggregate as not current", () => {
		expect(
			ComposerContextUsageIsCurrent(policy(), {
				context_tokens: 128_000,
				context_window_tokens: 258_400,
				scope: "run",
				scope_id: "run_1",
			}),
		).toBe(false);
	});

	/** The gauge hides entirely rather than pairing a stale numerator with a fresh window. */
	it("gates the composer's whole reading on the origin matching the policy", () => {
		const controls = read("modules/frontend/src/routes/components/composer/controls.svelte");

		expect(controls).toContain("ComposerContextUsageIsCurrent(policy, context_usage)");
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
	/**
	 * Sending a message starts a run, which selects that run's usage and leaves
	 * the controller `Loading` until it first reports. Blanking on anything that
	 * was not `Ready` took the gauge down for exactly the stretch the reader is
	 * watching it — from pressing send until the model answers — so it read as
	 * the gauge flickering out rather than a number being fetched.
	 */
	it("keeps the last reading on screen while the next run's usage loads", () => {
		const route = read("modules/frontend/src/routes/components/thread-route.svelte");

		expect(route).toContain('if (state._tag === "Loading") return;');
		expect(route).not.toContain(
			'context_usage = state._tag === "Ready" ? state.aggregate : undefined;',
		);
		/** A run that cannot report, or no run at all, still takes it down. */
		expect(route).toContain("context_usage = undefined;");
	});
});
