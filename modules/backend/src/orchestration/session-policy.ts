import { model_manifest } from "@artisan/catalog";
import type { EnginePermissionPolicy, EngineRunMetadata } from "@artisan/engines";
import type { ThreadSessionPolicy } from "@artisan/protocol";

/** Returns whether a requested executable is permitted by the thread policy. */
export const IsSessionPolicyEngine = (policy: ThreadSessionPolicy, engine_id: string) =>
	policy.engine_id === engine_id;

type NeutralPermissionOptionId = "autonomous" | "restricted" | "supervised";

/**
 * Resolves a harness's native permission vocabulary from the catalog, so the
 * manifest stays the single source of provider wording. `default` is Claude's
 * own supervised mode and the safe fallback for an unknown option.
 */
const native_permission_mode = (harness_id: string, option_id: NeutralPermissionOptionId) =>
	model_manifest.harnesses
		.find((harness) => harness.id === harness_id)
		?.permissions.options.find((option) => option.id === option_id)?.native_value ?? "default";

/**
 * Resolves the catalog's default model for a harness when policy and request
 * both leave it unset. A run must receive Artisan's catalog default
 * explicitly rather than silently inheriting whatever the operator's personal
 * CLI configuration defaults to (e.g. `~/.claude`'s own last-used model).
 * Returns `undefined` when the harness has no enabled catalog model, in which
 * case the field is omitted as before — an engine with no catalog models
 * cannot be defaulted.
 */
const catalog_default_model = (harness_id: string) =>
	model_manifest.models.find(
		(model) => model.harness === harness_id && model.disabled === undefined,
	)?.native_model_id;

/**
 * Resolves the executable-facing subset of durable session policy.
 *
 * Assignment permissions may narrow a user policy, but never widen it. This
 * preserves deliberately constrained graph work while keeping the thread's
 * sandbox, approval, and network choices authoritative.
 */
export const MakeSessionPolicyRunMetadata = (
	policy: ThreadSessionPolicy,
	requested: Pick<EngineRunMetadata, "model" | "permission_policy"> = {},
): EngineRunMetadata => {
	const requested_permissions = requested.permission_policy;
	const write_access =
		policy.sandbox_mode === "workspace_write" && (requested_permissions?.write_access ?? true);
	const network_access =
		write_access &&
		policy.web_search_enabled &&
		(requested_permissions?.network_access ?? true);
	const approval: EnginePermissionPolicy["approval"] =
		policy.permission_mode === "never" || requested_permissions?.approval === "never"
			? "never"
			: "on_request";
	const resolved_model =
		policy.model ?? requested.model ?? catalog_default_model(policy.engine_id);
	const model_metadata: Pick<EngineRunMetadata, "model"> =
		resolved_model === undefined ? {} : { model: resolved_model };

	/**
	 * The Claude adapter has no native mapping for a canonical permission
	 * policy and accepts only its own permission-mode vocabulary, so the
	 * narrowed neutral outcome is translated through the catalog instead.
	 */
	if (policy.engine_id === "claude") {
		const neutral_option: NeutralPermissionOptionId = !write_access
			? "restricted"
			: approval === "never"
				? "autonomous"
				: "supervised";
		return {
			...model_metadata,
			provider_options: {
				"claude.permission_mode": native_permission_mode("claude", neutral_option),
			},
		};
	}

	return {
		...model_metadata,
		permission_policy: { approval, network_access, write_access },
		provider_options: {
			"codex.reasoning_effort": policy.reasoning_effort,
			"codex.service_tier": policy.service_tier ?? "standard",
		},
	};
};
