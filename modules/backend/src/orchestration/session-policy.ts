import type { EnginePermissionPolicy, EngineRunMetadata } from "@artisan/engines";
import type { ThreadSessionPolicy } from "@artisan/protocol";

/** Returns whether a requested executable is permitted by the thread policy. */
export const IsSessionPolicyEngine = (policy: ThreadSessionPolicy, engine_id: string) =>
	policy.engine_id === engine_id;

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

	return {
		...(policy.model === undefined && requested.model === undefined
			? {}
			: { model: policy.model ?? requested.model }),
		permission_policy: { approval, network_access, write_access },
		provider_options: {
			"codex.reasoning_effort": policy.reasoning_effort,
			"codex.service_tier": policy.service_tier ?? "standard",
			"codex.workflow_mode": policy.workflow_mode ?? "build",
		},
	};
};
