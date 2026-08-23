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
	it("preserves Hermes provider groups and session runtime controls", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({
				catalog_revision: "hermes-live-1",
				engine_id: "hermes",
				model: "anthropic/claude-sonnet-4.6",
				model_id: "anthropic/claude-sonnet-4.6",
				permission: "supervised",
				profile_id: "default",
				provider_route_id: "nous",
			}),
		);

		expect(metadata).toMatchObject({
			catalog_revision: "hermes-live-1",
			model: "anthropic/claude-sonnet-4.6",
			model_id: "anthropic/claude-sonnet-4.6",
			permission_policy: {
				approval: "on_request",
				edit_scope: "host",
				network_access: true,
				write_access: true,
			},
			profile_id: "default",
			provider_options: {
				"hermes.fast": false,
				"hermes.permission_mode": "profile",
				"hermes.reasoning_effort": "medium",
			},
			provider_route_id: "nous",
		});
	});

	it("preserves OpenCode route, profile, variant, and catalog identity", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({
				catalog_revision: "live-1",
				engine_id: "opencode2",
				model: "claude-sonnet-4-5",
				model_id: "claude-sonnet-4-5",
				profile_id: "work",
				provider_route_id: "opencode-go",
				variant_id: "high",
				web_search_enabled: true,
			}),
		);

		expect(metadata).toMatchObject({
			catalog_revision: "live-1",
			model: "claude-sonnet-4-5",
			model_id: "claude-sonnet-4-5",
			permission_policy: {
				approval: "on_request",
				network_access: true,
				write_access: true,
			},
			profile_id: "work",
			provider_route_id: "opencode-go",
			provider_options: {
				"opencode2.agent": "artisan-v1-autonomous-network-web",
				"opencode2.project_config": false,
				"opencode2.web_search_enabled": true,
			},
			variant_id: "high",
		});
	});

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

	it("keeps Auto sandboxed while mapping Unrestricted to host scope", () => {
		const auto_approve = MakeSessionPolicyRunMetadata(
			policy_for({ permission: "autonomous", permission_mode: "never" }),
		);
		const full_access = MakeSessionPolicyRunMetadata(
			policy_for({ permission: "unrestricted", permission_mode: "never" }),
		);
		const narrowed_assignment = MakeSessionPolicyRunMetadata(
			policy_for({ permission: "unrestricted", permission_mode: "never" }),
			{
				permission_policy: {
					approval: "never",
					network_access: false,
					write_access: true,
				},
			},
		);
		const approval_narrowed_assignment = MakeSessionPolicyRunMetadata(
			policy_for({ permission: "unrestricted", permission_mode: "never" }),
			{
				permission_policy: {
					approval: "on_request",
					edit_scope: "host",
					network_access: true,
					write_access: true,
				},
			},
		);
		const approval_widening_attempt = MakeSessionPolicyRunMetadata(policy_for(), {
			permission_policy: {
				approval: "never",
				network_access: false,
				write_access: true,
			},
		});

		expect(auto_approve.permission_policy).toEqual({
			approval: "on_request",
			network_access: false,
			write_access: true,
		});
		expect(full_access.permission_policy).toEqual({
			approval: "never",
			edit_scope: "host",
			network_access: true,
			write_access: true,
		});
		expect(narrowed_assignment.permission_policy).toEqual({
			approval: "never",
			network_access: false,
			write_access: true,
		});
		expect(approval_narrowed_assignment.permission_policy).toEqual({
			approval: "on_request",
			edit_scope: "host",
			network_access: true,
			write_access: true,
		});
		expect(approval_widening_attempt.permission_policy).toEqual({
			approval: "on_request",
			network_access: false,
			write_access: true,
		});
	});

	it("translates the neutral outcome into Claude's native permission mode", () => {
		const auto = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "claude",
				model: "claude-opus-5",
				permission: "autonomous",
			}),
		);
		const legacy_supervised = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "claude",
				model: "claude-opus-5",
				permission: "supervised",
			}),
		);
		const restricted = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5", sandbox_mode: "read_only" }),
		);
		/** The adapter rejects canonical policies, so none may be emitted. */
		for (const metadata of [auto, legacy_supervised, restricted]) {
			expect(metadata.permission_policy).toBeUndefined();
			expect(metadata.provider_options?.["claude.effort"]).toBe("medium");
			expect(Object.keys(metadata.provider_options ?? {}).sort()).toEqual([
				"claude.effort",
				"claude.permission_mode",
			]);
		}
		expect(auto.provider_options?.["claude.permission_mode"]).toBe("auto");
		expect(legacy_supervised.provider_options?.["claude.permission_mode"]).toBe("auto");
		expect(restricted.provider_options?.["claude.permission_mode"]).toBe("plan");
		expect(auto.model).toBe("claude-opus-5");
	});

	it("translates Grok and Cursor model controls into their ACP launch options", () => {
		const grok_restricted = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "grok",
				model: "grok-4.6",
				permission: "restricted",
				sandbox_mode: "read_only",
			}),
		);
		const grok_legacy_trusted = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "grok",
				model: "grok-4.6",
				permission: "trusted",
			}),
		);
		const grok = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "grok",
				model: "grok-4.6",
				permission: "autonomous",
				reasoning_effort: "xhigh",
				web_search_enabled: true,
			}),
		);
		const cursor_restricted = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "cursor",
				model: "cursor-grok-4.6",
				permission: "restricted",
				permission_mode: "never",
				sandbox_mode: "read_only",
			}),
		);
		const cursor = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "cursor",
				model: "cursor-grok-4.6",
				permission: "unrestricted",
				permission_mode: "never",
				reasoning_effort: "high",
				service_tier: "fast",
			}),
		);

		expect(grok_restricted).toMatchObject({
			permission_policy: { approval: "on_request", write_access: false },
			provider_options: { "grok.permission_mode": "plan" },
		});
		expect(grok_legacy_trusted).toMatchObject({
			permission_policy: { approval: "on_request", write_access: true },
			provider_options: { "grok.permission_mode": "auto" },
		});
		expect(grok).toMatchObject({
			model: "grok-4.6",
			permission_policy: { approval: "on_request", write_access: true },
			provider_options: {
				"grok.permission_mode": "auto",
				"grok.reasoning_effort": "xhigh",
			},
		});
		expect(cursor_restricted).toMatchObject({
			permission_policy: { approval: "never", write_access: false },
			provider_options: { "cursor.permission_mode": "ask" },
		});
		expect(cursor).toMatchObject({
			model: "cursor-grok-4.6",
			permission_policy: {
				approval: "never",
				edit_scope: "host",
				network_access: true,
				write_access: true,
			},
			provider_options: {
				"cursor.permission_mode": "force",
				"cursor.reasoning_effort": "high",
				"cursor.speed": "fast",
			},
		});
		expect(grok.provider_options).not.toHaveProperty("codex.reasoning_effort");
		expect(cursor.provider_options).not.toHaveProperty("codex.service_tier");
	});

	it("passes Claude's catalog-validated special effort to the native CLI boundary", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "claude",
				model: "claude-opus-5",
				reasoning_effort: "max",
			}),
		);

		expect(metadata.provider_options?.["claude.effort"]).toBe("max");
	});

	it("omits Claude's effort option when the selected catalog model does not support it", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "claude",
				model: "claude-haiku-4-5",
				reasoning_effort: "medium",
			}),
		);

		expect(metadata.provider_options).toEqual({
			"claude.permission_mode": "auto",
		});
	});

	it("normalizes retired choices while preserving Unrestricted", () => {
		const legacy_trusted = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5", permission: "trusted" }),
		);
		const unrestricted = MakeSessionPolicyRunMetadata(
			policy_for({
				engine_id: "claude",
				model: "claude-opus-5",
				permission: "unrestricted",
				permission_mode: "never",
			}),
		);

		expect(legacy_trusted.provider_options?.["claude.permission_mode"]).toBe("auto");
		expect(unrestricted.provider_options?.["claude.permission_mode"]).toBe("bypassPermissions");
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

	it("falls back to the catalog's default model instead of the operator's personal CLI default", () => {
		const codex_default = MakeSessionPolicyRunMetadata(policy_for({ model: undefined }));
		const claude_default = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: undefined }),
		);

		expect(codex_default.model).toBe("gpt-5.6-sol");
		expect(claude_default.model).toBe("claude-fable-5");
	});

	it("keeps an explicit request model when the policy leaves the model unset", () => {
		const metadata = MakeSessionPolicyRunMetadata(policy_for({ model: undefined }), {
			model: "gpt-5.5",
		});

		expect(metadata.model).toBe("gpt-5.5");
	});

	it("leaves the model unset when the policy explicitly requests one, unchanged", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({ engine_id: "claude", model: "claude-opus-5" }),
		);

		expect(metadata.model).toBe("claude-opus-5");
	});

	/** Claude names its window in the model id, and that spelling must survive. */
	it("composes Claude's extended window onto the model id", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({ context_window: "[1m]", engine_id: "claude", model: "claude-opus-5" }),
		);

		expect(metadata.model).toBe("claude-opus-5[1m]");
	});

	/**
	 * Codex resolves every GPT-5 model to 272K and takes a larger window as
	 * configuration, so its extended option must reach the run as a config value
	 * and must not touch the model id — `gpt-5.6-sol1m` is not a model Codex can
	 * resolve, and a run asking for one fails to start at all.
	 */
	it("sends Codex's extended window as configuration and leaves its model id alone", () => {
		const metadata = MakeSessionPolicyRunMetadata(policy_for({ context_window: "1m" }));

		expect(metadata.model).toBe("gpt-5.6-sol");
		expect(metadata.provider_options?.["codex.model_context_window"]).toBe("1050000");
	});

	it("sends no window configuration for the standard Codex window", () => {
		const metadata = MakeSessionPolicyRunMetadata(policy_for());

		expect(metadata.model).toBe("gpt-5.6-sol");
		expect(metadata.provider_options?.["codex.model_context_window"]).toBeUndefined();
	});

	/**
	 * A token written before configurable windows existed can only have meant a
	 * suffix, so an unrecognized one is still appended rather than dropped —
	 * dropping it would quietly downgrade those threads to the base window.
	 */
	it("still appends a window token the catalog no longer publishes", () => {
		const metadata = MakeSessionPolicyRunMetadata(
			policy_for({ context_window: "[2m]", engine_id: "claude", model: "claude-opus-5" }),
		);

		expect(metadata.model).toBe("claude-opus-5[2m]");
	});
});
