import { ComposeNativeModelId, ContextWindowNativeConfig, model_manifest } from "@artisan/catalog";
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

type PermissionEditScope = "none" | "workspace" | "host";

const permission_scope_order: Readonly<Record<PermissionEditScope, number>> = {
	none: 0,
	workspace: 1,
	host: 2,
};

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
 * Resolves a policy effort only when the selected catalog model publishes it.
 * Models with native or unavailable thinking intentionally receive no effort
 * option, even though the durable policy retains its last model-level choice.
 */
const catalog_native_effort = (
	harness_id: string,
	native_model_id: string | undefined,
	reasoning_effort: ThreadSessionPolicy["reasoning_effort"],
) => {
	const thinking = model_manifest.models.find(
		(model) => model.harness === harness_id && model.native_model_id === native_model_id,
	)?.capabilities.thinking;
	if (thinking?.availability !== "supported") return undefined;
	return thinking.options.find((option) => option.native_value === reasoning_effort)
		?.native_value;
};

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
	const policy_edit_scope: PermissionEditScope =
		chosen === undefined
			? policy.sandbox_mode === "workspace_write"
				? "workspace"
				: "none"
			: chosen.edit_scope;
	/**
	 * Existing canonical callers that request writes mean workspace writes.
	 * Host access must be explicit, and assignment policy can only narrow the
	 * user's catalog choice.
	 */
	const requested_edit_scope: PermissionEditScope =
		requested_permissions === undefined
			? policy_edit_scope
			: requested_permissions.write_access
				? (requested_permissions.edit_scope ?? "workspace")
				: "none";
	const narrowed_edit_scope =
		permission_scope_order[requested_edit_scope] < permission_scope_order[policy_edit_scope]
			? requested_edit_scope
			: policy_edit_scope;
	/**
	 * Codex's host-wide sandbox mode also removes network isolation. A caller
	 * that narrows network access therefore narrows filesystem access back to
	 * the workspace boundary as well.
	 */
	const edit_scope: PermissionEditScope =
		narrowed_edit_scope === "host" && requested_permissions?.network_access === false
			? "workspace"
			: narrowed_edit_scope;
	const write_access = edit_scope !== "none";
	const network_access =
		edit_scope === "host" ||
		(write_access &&
			policy.web_search_enabled &&
			(requested_permissions?.network_access ?? true));
	const policy_approval: EnginePermissionPolicy["approval"] = (
		chosen === undefined
			? policy.permission_mode === "never"
			: chosen.approval_behavior === "none"
	)
		? "never"
		: "on_request";
	const approval: EnginePermissionPolicy["approval"] =
		requested_permissions === undefined
			? policy_approval
			: policy_approval === "on_request" || requested_permissions.approval !== "never"
				? "on_request"
				: "never";
	/**
	 * The context-window choice travels as a native model-id suffix (for
	 * example Claude Code's `[1m]`), so the engine sees one composed id — except
	 * where the harness treats the window as configuration, which the catalog
	 * knows and this composition defers to. Codex is that case: appending to its
	 * model id would name a model its own catalog cannot resolve.
	 */
	const resolved_model = SessionPolicyResolvedModel(policy, requested.model);
	const model_metadata: Pick<EngineRunMetadata, "model"> =
		resolved_model === undefined
			? {}
			: {
					model: ComposeNativeModelId(
						policy.engine_id,
						resolved_model,
						policy.context_window,
					),
				};

	/**
	 * The Claude adapter has no native mapping for a canonical permission
	 * policy and accepts only its own permission-mode vocabulary, so the
	 * narrowed neutral outcome is translated through the catalog instead.
	 */
	if (policy.engine_id === "claude") {
		const effort = catalog_native_effort("claude", resolved_model, policy.reasoning_effort);
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
				...(effort === undefined ? {} : { "claude.effort": effort }),
				"claude.permission_mode": native_permission_mode("claude", narrowed_option),
			},
		};
	}

	/**
	 * Codex resolves every GPT-5 model to 272K and compacts at nine tenths of
	 * whatever it resolved, so the extended window is a config override rather
	 * than a different model. Sent as a string because provider options are the
	 * engine-neutral string channel; the adapter parses it back at the boundary
	 * that owns Codex's own config shape.
	 */
	const native_config = ContextWindowNativeConfig(
		policy.engine_id,
		resolved_model,
		policy.context_window,
	);

	return {
		...model_metadata,
		permission_policy: {
			approval,
			...(edit_scope === "host" ? { edit_scope } : {}),
			network_access,
			write_access,
		},
		provider_options: {
			...(native_config === undefined
				? {}
				: {
						"codex.model_context_window": String(native_config.model_context_window),
					}),
			"codex.reasoning_effort": policy.reasoning_effort,
			"codex.service_tier": policy.service_tier ?? "standard",
		},
	};
};
