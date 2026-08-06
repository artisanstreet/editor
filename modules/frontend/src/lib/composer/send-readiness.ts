import type { RuntimeCatalog, SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";

/**
 * Sending needs a live engine behind it. Without Forge there is no session to
 * run at all, and within a connected catalog a model whose harness is
 * unregistered can still be picked and read but never run.
 */
export const ComposerSendBlockedReason = (
	forge_available: boolean,
	catalog: RuntimeCatalog,
	policy: ThreadSessionPolicy | undefined,
): string | undefined => {
	if (!forge_available) return "Forge is offline — reconnect to send";
	const engine_id = policy?.engine_id;
	if (engine_id === undefined) return undefined;
	if (catalog.runnable_harness_ids.includes(engine_id)) return undefined;
	const label =
		catalog.manifest.harnesses.find((harness) => harness.id === engine_id)?.label ?? engine_id;
	return `${label} models are preview-only — this engine cannot run in Artisan yet`;
};

/**
 * The context-window denominator. A provider that discloses its usable window
 * on the wire (Codex) wins; otherwise the catalog's configured context-window
 * option for the thread's model stands in (Claude never reports a window
 * size). Models without the capability show no gauge.
 *
 * A policy without a context-window choice resolves to the capability's
 * default option, never the suffix-less one. The launcher composes the model
 * id as `model + (suffix ?? "")`, and the harness resolves a bare Claude 5 id
 * to its extended window — live sessions measured 236K+ tokens of context on
 * plain model ids. Dividing those readings by the 200K "standard" option is
 * what pinned the gauge at 100% for turns on end.
 */
export const ComposerContextWindowTokens = (
	catalog: RuntimeCatalog,
	policy: ThreadSessionPolicy | undefined,
	context_usage: SurfaceUsageAggregate | undefined,
): number | undefined => {
	if (context_usage?.context_window_tokens !== undefined) {
		return context_usage.context_window_tokens;
	}
	const capability = catalog.manifest.models.find(
		(model) => model.native_model_id === policy?.model,
	)?.capabilities.context_window;
	if (capability === undefined) return undefined;
	const default_option = capability.options.find(
		(candidate) => candidate.id === capability.default,
	);
	const option =
		policy?.context_window === undefined
			? default_option
			: (capability.options.find(
					(candidate) => candidate.native_suffix === policy.context_window,
				) ?? default_option);
	return option?.tokens;
};
