import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import type { SurfaceUsageAggregate } from "@artisan/protocol";
import {
	ContextAutoCompactionPercent,
	ContextUsageAutoCompactionPercent,
} from "../../modules/frontend/src/lib/context-usage/auto-compaction";
import { ContextUsageModelName } from "../../modules/frontend/src/lib/context-usage/model-name";
import { OfflineRuntimeCatalog } from "../../modules/frontend/src/lib/runtime/offline-catalog";

const usage = (
	context_origin?: SurfaceUsageAggregate["context_origin"],
): SurfaceUsageAggregate => ({
	context_tokens: 128_000,
	context_window_tokens: 258_400,
	...(context_origin === undefined ? {} : { context_origin }),
	scope: "run",
	scope_id: "run_luna",
});

describe("context usage model name", () => {
	it("keeps a Luna reporting run's name and threshold when the next policy selects Sol", () => {
		const reporting_usage = usage({
			engine_id: "codex",
			model_id: "gpt-5.6-luna",
			run_id: "run_luna",
		});
		const next_launch_policy = { engine_id: "claude", model: "claude-sonnet-5" };
		const model_name = ContextUsageModelName(reporting_usage, OfflineRuntimeCatalog);

		expect(
			ContextAutoCompactionPercent({
				harness: next_launch_policy.engine_id,
				native_model_id: next_launch_policy.model,
				window_tokens: 258_400,
			}),
		).toBe(100);
		expect(model_name).toBe("GPT 5.6 Luna");
		expect(ContextUsageAutoCompactionPercent(reporting_usage, 258_400)).toBe(90);
	});

	it("uses truthful generic tooltip copy when provenance cannot name a catalog model", () => {
		const details = readFileSync(
			resolve(
				import.meta.dirname,
				"../../modules/frontend/src/routes/components/context-usage-details.svelte",
			),
			"utf8",
		);

		expect(
			ContextUsageModelName(
				usage({ engine_id: "codex", model_id: "retired", run_id: "run_old" }),
				OfflineRuntimeCatalog,
			),
		).toBeUndefined();
		expect(ContextUsageModelName(usage(), OfflineRuntimeCatalog)).toBeUndefined();
		expect(ContextUsageAutoCompactionPercent(usage(), 258_400)).toBe(100);
		expect(details).toContain('model_name ?? "this model"');
	});
});
