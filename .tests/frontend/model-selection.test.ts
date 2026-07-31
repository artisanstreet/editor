import { describe, expect, it } from "vitest";

import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog, ThreadSessionPolicy } from "@artisan/protocol";
import {
	permission_for_harness,
	permission_policy_for_harness,
	permission_policy_matches,
	permission_reconciliation_for_harness,
	policy_fields_for_permission,
} from "../../modules/frontend/src/lib/engine/model-selection";

const catalog = {
	manifest: model_manifest,
	runnable_harness_ids: ["codex", "claude"],
} satisfies RuntimeCatalog;

describe("model permission selection", () => {
	it("restores Full access with its no-prompt compatibility axes", () => {
		const option = permission_for_harness(catalog, "codex", "unrestricted");

		expect(option?.native_value).toBe("danger-full-access");
		expect(policy_fields_for_permission(option)).toEqual({
			permission: "unrestricted",
			permission_mode: "never",
			sandbox_mode: "workspace_write",
		});
	});

	it("falls back when a saved permission is unsupported by the selected harness", () => {
		const resolved = permission_policy_for_harness(catalog, "codex", "trusted");

		expect(resolved.option?.id).toBe("supervised");
		expect(resolved.fields).toEqual({
			permission: "supervised",
			permission_mode: "on_request",
			sandbox_mode: "workspace_write",
		});
		expect(resolved.option?.id).toBe(resolved.fields.permission);
	});

	it("keeps read-only policy axes aligned with the catalog", () => {
		const option = permission_for_harness(catalog, "codex", "restricted");

		expect(policy_fields_for_permission(option)).toEqual({
			permission: "restricted",
			permission_mode: "never",
			sandbox_mode: "read_only",
		});
	});

	it("repairs stale compatibility axes even when the permission id is valid", () => {
		const stale_policy: ThreadSessionPolicy = {
			engine_id: "codex",
			model: "gpt-5.6-codex",
			permission: "unrestricted",
			permission_mode: "on_request",
			reasoning_effort: "medium",
			sandbox_mode: "workspace_write",
			service_tier: "standard",
			strict_clarification: false,
			web_search_enabled: false,
		};
		const resolved = permission_reconciliation_for_harness(catalog, "codex", stale_policy);

		expect(resolved.needs_update).toBe(true);
		const reconciled = { ...stale_policy, ...resolved.fields };
		expect(permission_policy_matches(reconciled, resolved.fields)).toBe(true);
		expect(reconciled.permission_mode).toBe("never");
	});
});
