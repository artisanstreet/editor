import { model_manifest } from "./model-manifest";
import type { ContextWindowOption } from "./schema";

/**
 * Resolves the context-window option a stored policy token names.
 *
 * The token is the option's `native_suffix`, which is what every writer and
 * reader of `ThreadSessionPolicy.context_window` has always stored. Options
 * whose window is configuration rather than identity still carry one — it just
 * names the option instead of describing a model id.
 */
export const ContextWindowOptionFor = (
	harness_id: string,
	native_model_id: string | undefined,
	stored_token: string | undefined,
): ContextWindowOption | undefined => {
	if (stored_token === undefined || native_model_id === undefined) return undefined;
	return model_manifest.models
		.find((model) => model.harness === harness_id && model.native_model_id === native_model_id)
		?.capabilities.context_window?.options.find(
			(option) => option.native_suffix === stored_token,
		);
};

/**
 * Composes the native model id a run is dispatched with.
 *
 * A window expressed as configuration must not reach the model id: Codex has
 * no `gpt-5.6-sol1m` in its catalog and would fail to resolve one. An unknown
 * token is still appended, because that is what every stored policy written
 * before configurable windows existed means, and dropping it would silently
 * downgrade those threads to the base window.
 */
export const ComposeNativeModelId = (
	harness_id: string,
	native_model_id: string,
	stored_token: string | undefined,
): string => {
	if (stored_token === undefined) return native_model_id;
	const option = ContextWindowOptionFor(harness_id, native_model_id, stored_token);
	return option?.native_config === undefined
		? `${native_model_id}${stored_token}`
		: native_model_id;
};

/** The harness configuration a stored window token selects, when it is one. */
export const ContextWindowNativeConfig = (
	harness_id: string,
	native_model_id: string | undefined,
	stored_token: string | undefined,
) => ContextWindowOptionFor(harness_id, native_model_id, stored_token)?.native_config;
