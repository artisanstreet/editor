import { model_manifest, model_reasoning_display, type ReasoningDisplay } from "@artisan/catalog";
import type { ThreadSessionPolicy } from "@artisan/protocol";

/**
 * Resolves how the thread's own model presents its thinking, which decides
 * whether the live thinking line may say a summary or must keep a verb.
 *
 * An unresolvable model reads as a summary engine. Every engine Artisan runs
 * today publishes summaries, and the failure the other default would cause is
 * the worse one: a model whose thinking is perfectly presentable would be
 * silenced behind a verb for the whole run, with nothing on screen to explain
 * why. A model that genuinely streams raw thought says so in the catalog.
 */
export const policy_reasoning_display = (
	policy: ThreadSessionPolicy | undefined,
): ReasoningDisplay => {
	if (policy?.model === undefined) return "summary";
	const definition = model_manifest.models.find(
		(model) =>
			model.harness === policy.engine_id &&
			(model.native_model_id === policy.model || model.id === policy.model),
	);
	return definition === undefined ? "summary" : model_reasoning_display(definition);
};
