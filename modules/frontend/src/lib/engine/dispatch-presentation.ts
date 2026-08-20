import { model_manifest } from "@artisan/catalog";
import type { ThreadSessionPolicy } from "@artisan/protocol";

import { thinking_level_labels } from "./model-selection";
import { ProviderMarkFor, type EngineMark } from "./presentation";
import { speed_option_presentation, type SpeedOptionPresentation } from "./speed-presentation";

export interface DispatchModelPresentation {
	/**
	 * The whole dispatch in words. Surfaces that drop qualifiers to fit their
	 * width hide them outright, which takes them from assistive technology too;
	 * this stays complete no matter how little is drawn.
	 */
	readonly aria_label: string;
	readonly effort_label: string | undefined;
	/**
	 * The mark of the lab that made the model, not the engine serving it, so a
	 * Cursor-hosted GPT still reads as OpenAI. It is what the name collapses to
	 * where a row is too narrow to spell anything out.
	 */
	readonly mark: EngineMark;
	readonly model_name: string;
	readonly speed: SpeedOptionPresentation | undefined;
}

/**
 * Names what a dispatched agent actually runs on, in the model picker's
 * vocabulary: the catalog model name, the thinking label when the model
 * supports selectable effort, and the speed tier only when it is not the
 * model's own default — every row would otherwise trail a "Standard" that
 * says nothing.
 *
 * Mirrors the backend's dispatch resolution (`SessionPolicyResolvedModel`):
 * the thread policy's model wins over the assignment's requested profile, and
 * effort and speed always come from the thread policy because assignments
 * cannot override them. A profile that names no catalog model (such as the
 * adopted-native `provider-native` marker with no policy model behind it)
 * yields no presentation rather than a guessed one.
 */
export const dispatch_model_presentation = (
	policy: ThreadSessionPolicy | undefined,
	engine_id: string,
	profile: string,
): DispatchModelPresentation | undefined => {
	if (policy === undefined) return undefined;
	const requested = policy.model ?? profile;
	const definition = model_manifest.models.find(
		(model) =>
			model.harness === engine_id &&
			(model.native_model_id === requested || model.id === requested),
	);
	if (definition === undefined) return undefined;
	const thinking = definition.capabilities.thinking;
	const effort_option =
		thinking.availability === "supported"
			? thinking.options.find((option) => option.native_value === policy.reasoning_effort)
			: undefined;
	const speed_option = definition.capabilities.speed_options.find(
		(option) => option.native_value === policy.service_tier,
	);

	const effort_label =
		effort_option === undefined ? undefined : thinking_level_labels[effort_option.id];
	const speed =
		speed_option === undefined || speed_option.default
			? undefined
			: speed_option_presentation(speed_option);

	return {
		aria_label: [definition.name, effort_label, speed?.label].filter(Boolean).join(" "),
		effort_label,
		mark: ProviderMarkFor(definition.provider),
		model_name: definition.name,
		speed,
	};
};
