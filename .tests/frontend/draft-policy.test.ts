import { describe, expect, it } from "vitest";

import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog, SessionDefaults } from "@artisan/protocol";
import { SeededDraftPolicy } from "../../modules/frontend/src/lib/root/draft-policy";

const catalog = {
	default_model_id: "codex-sol",
	manifest: model_manifest,
	runnable_harness_ids: ["codex", "cursor"],
} satisfies RuntimeCatalog;

const defaults = (patch: Partial<SessionDefaults> = {}): SessionDefaults => ({
	models: [],
	permission: "supervised",
	...patch,
});

describe("new-thread draft policy", () => {
	it("restores the exact last catalog model with its effort and fast tier", () => {
		const policy = SeededDraftPolicy(
			catalog,
			defaults({
				last_model_id: "codex-sol",
				models: [
					{
						model_id: "codex-sol",
						reasoning_effort: "xhigh",
						service_tier: "fast",
					},
				],
			}),
		);

		expect(policy).toMatchObject({
			engine_id: "codex",
			model: "gpt-5.6-sol",
			reasoning_effort: "xhigh",
			service_tier: "fast",
		});
	});

	it("uses the catalog id to distinguish models with the same provider-native id", () => {
		const policy = SeededDraftPolicy(
			catalog,
			defaults({ last_model_id: "cursor-gpt-5-6-sol" }),
		);

		expect(policy.engine_id).toBe("cursor");
		expect(policy.model).toBe("gpt-5.6-sol");
	});

	it("accepts a legacy native last-model id and falls back from an unavailable speed", () => {
		const legacy = SeededDraftPolicy(catalog, defaults({ last_model_id: "gpt-5.6-sol" }));
		const unavailable_speed = SeededDraftPolicy(
			catalog,
			defaults({
				last_model_id: "codex-gpt-5-4-mini",
				models: [{ model_id: "codex-gpt-5-4-mini", service_tier: "fast" }],
			}),
		);

		expect(legacy).toMatchObject({ engine_id: "codex", model: "gpt-5.6-sol" });
		expect(unavailable_speed.service_tier).toBe("standard");
	});
});
