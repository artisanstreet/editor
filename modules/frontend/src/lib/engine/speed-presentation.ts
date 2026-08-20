import type { SpeedOption } from "./model-selection";

export interface SpeedOptionPresentation {
	readonly class_name: string;
	readonly label: string;
}

const fast_presentation: SpeedOptionPresentation = {
	class_name: "text-amber-600 dark:text-amber-400",
	label: "Fast",
};

const superfast_presentation: SpeedOptionPresentation = {
	class_name: "bg-linear-to-r from-purple-500 to-green-500 bg-clip-text text-transparent",
	label: "Superfast",
};

/**
 * Gives accelerated inference tiers a compact model-picker trigger treatment.
 * The policy dropdown keeps ordinary catalog text so color does not compete
 * with its selection and tooltip states. Future Cerebras-backed GPT catalog
 * entries use the stable `superfast` tier ID; the provider name stays catalog
 * metadata rather than UI branching input.
 */
export const speed_option_presentation = (
	option: Pick<SpeedOption, "id" | "label">,
): SpeedOptionPresentation => {
	if (option.id === "fast") return fast_presentation;
	if (option.id === "superfast") return superfast_presentation;
	return { class_name: "", label: option.label };
};
