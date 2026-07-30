import { model_manifest } from "@artisan/catalog";
import type { EnginePermissionPolicy, EngineRunMetadata } from "@artisan/engines";
import { SessionPolicyPermission, type ThreadSessionPolicy } from "@artisan/protocol";

/** Returns whether a requested executable is permitted by the thread policy. */
export const IsSessionPolicyEngine = (policy: ThreadSessionPolicy, engine_id: string) =>
	policy.engine_id === engine_id;

/**
 * Resolves a harness's permission option from the catalog, so the manifest
 * stays the single source of both provider wording and the coarse sandbox and
 * approval axes each neutral option implies.
 */
const permission_option = (harness_id: string, option_id: string) =>
	model_manifest.harnesses
		.find((harness) => harness.id === harness_id)
		?.permissions.options.find((option) => option.id === option_id);

/**
 * Resolves a harness's native permission vocabulary from the catalog. `default`
 * is Claude's own supervised mode and the safe fallback for an unknown option.
 */
const native_permission_mode = (harness_id: string, option_id: string) =>
	permission_option(harness_id, option_id)?.native_value ?? "default";

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
 * Resolves the catalog model a run will be dispatched with, before the
 * context-window suffix is composed onto the native id. This is the value to
 * persist for usage attribution: it matches the manifest's `native_model_id`
 * and stays joinable after the policy later changes.
 */
export const SessionPolicyResolvedModel = (
	policy: ThreadSessionPolicy,
	requested_model?: string,
): string | undefined => policy.model ?? requested_model ?? catalog_default_model(policy.engine_id);

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
	/**
	 * The option the user chose is authoritative; the coarse axes only stand in
	 * when the policy names an option this harness does not publish.
	 */
	const chosen_permission = SessionPolicyPermission(policy);
	const chosen = permission_option(policy.engine_id, chosen_permission);
	const policy_writes =
		chosen === undefined
			? policy.sandbox_mode === "workspace_write"
			: chosen.edit_scope !== "none";
	const write_access = policy_writes && (requested_permissions?.write_access ?? true);
	const network_access =
		write_access &&
		policy.web_search_enabled &&
		(requested_permissions?.network_access ?? true);
	const policy_approval: EnginePermissionPolicy["approval"] = (
		chosen === undefined
			? policy.permission_mode === "never"
			: chosen.approval_behavior === "none"
	)
		? "never"
		: "on_request";
	const approval: EnginePermissionPolicy["approval"] =
		policy_approval === "never" || requested_permissions?.approval === "never"
			? "never"
			: "on_request";
	/**
	 * The context-window choice travels as a native model-id suffix (for
	 * example Claude Code's `[1m]`), so the engine sees one composed id.
	 */
	const resolved_model = SessionPolicyResolvedModel(policy, requested.model);
	const model_metadata: Pick<EngineRunMetadata, "model"> =
		resolved_model === undefined
			? {}
			: { model: `${resolved_model}${policy.context_window ?? ""}` };

	/**
	 * The Claude adapter has no native mapping for a canonical permission
	 * policy and accepts only its own permission-mode vocabulary, so the
	 * narrowed neutral outcome is translated through the catalog instead.
	 */
	if (policy.engine_id === "claude") {
		/**
		 * The chosen option survives verbatim unless an assignment narrowed a
		 * boundary; only then does it collapse to the neutral option that
		 * describes the narrowed outcome. Re-deriving it in every case would
		 * erase the options the coarse axes cannot express (accept edits, auto,
		 * bypass permissions).
		 */
		const narrowed_option = !write_access
			? "restricted"
			: policy_approval !== "never" && approval === "never"
				? "autonomous"
				: chosen_permission;
		return {
			...model_metadata,
			provider_options: {
				"claude.permission_mode": native_permission_mode("claude", narrowed_option),
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
