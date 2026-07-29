import { describe, expect, it } from "vitest";

import type { ThreadSessionPolicy } from "@artisan/protocol";
import { MakeSessionPolicyRunMetadata } from "../../modules/backend/src/orchestration/session-policy";

const policy_for = (patch: Partial<ThreadSessionPolicy> = {}): ThreadSessionPolicy => ({
	engine_id: "codex",
	model: "gpt-5.6-sol",
	permission_mode: "on_request",
	reasoning_effort: "medium",
	sandbox_mode: "workspace_write",
	service_tier: "standard",
	strict_clarification: false,
	web_search_enabled: false,
	...patch,
});

describe("session policy run metadata", () => {
	it("keeps the canonical policy and Codex provider options for Codex runs", () => {
		const metadata = MakeSessionPolicyRunMetadata(policy_for());

		expect(metadata.permission_policy).toEqual({
			approval: "on_request",
			network_access: false,
			write_access: true,
		});
		expect(metadata.provider_options).toEqual({
			"codex.reasoning_effort": "medium",
			"codex.service_tier": "standard",
		});
	});

	it("translates the neutral outcome into Claude's native permission mode", () => {
		const supervised = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5" }),
		);
		const restricted = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5", sandbox_mode: "read_only" }),
		);
		const autonomous = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5", permission_mode: "never" }),
		);

		/** The adapter rejects canonical policies, so none may be emitted. */
		for (const metadata of [supervised, restricted, autonomous]) {
			expect(metadata.permission_policy).toBeUndefined();
			expect(Object.keys(metadata.provider_options ?? {})).toEqual([
				"claude.permission_mode",
			]);
		}
		expect(supervised.provider_options?.["claude.permission_mode"]).toBe("default");
		expect(restricted.provider_options?.["claude.permission_mode"]).toBe("plan");
		expect(autonomous.provider_options?.["claude.permission_mode"]).toBe("auto");
		expect(supervised.model).toBe("claude-opus-5");
	});

	it("lets assignment permissions narrow but never widen a Claude policy", () => {
		const narrowed = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5" }),
			{
				permission_policy: {
					approval: "on_request",
					network_access: false,
					write_access: false,
				},
			},
		);

		expect(narrowed.provider_options?.["claude.permission_mode"]).toBe("plan");
	});
});
