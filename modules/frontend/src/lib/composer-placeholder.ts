import raw_placeholders from "@artisan/data/composer/placeholders.json";
import { Schema } from "effect";

const PlaceholderVocabulary = Schema.NonEmptyArray(Schema.NonEmptyString);
const fallback_placeholders = ["Do anything"] as const;

const LoadComposerPlaceholders = (): ReadonlyArray<string> => {
	try {
		const decoded = Schema.decodeUnknownSync(PlaceholderVocabulary)(raw_placeholders);
		const unique = [...new Set(decoded)];
		return unique.length === decoded.length && unique.every((item) => item.trim().length > 0)
			? unique
			: fallback_placeholders;
	} catch {
		return fallback_placeholders;
	}
};

export const ComposerPlaceholders = LoadComposerPlaceholders();

export interface ComposerPlaceholderState {
	readonly generation: number;
	readonly phrase: string;
	readonly visible: boolean;
}

export const PickComposerPlaceholder = (
	previous: string | undefined,
	random: () => number = Math.random,
	vocabulary: ReadonlyArray<string> = ComposerPlaceholders,
): string => {
	const choices = vocabulary.length > 0 ? vocabulary : fallback_placeholders;
	const unit = Math.min(0.999_999, Math.max(0, random()));
	let index = Math.floor(unit * choices.length);
	if (choices.length > 1 && choices[index] === previous) index = (index + 1) % choices.length;
	return choices[index] ?? fallback_placeholders[0];
};

export const MakeComposerPlaceholderState = (
	random: () => number = Math.random,
): ComposerPlaceholderState => ({
	generation: 0,
	phrase: PickComposerPlaceholder(undefined, random),
	visible: true,
});

export const UpdateComposerPlaceholderState = (
	state: ComposerPlaceholderState,
	value: string,
	random: () => number = Math.random,
): ComposerPlaceholderState => {
	const visible = value.length === 0;
	if (visible === state.visible) return state;
	if (!visible) return { ...state, visible: false };

	return {
		generation: state.generation + 1,
		phrase: PickComposerPlaceholder(state.phrase, random),
		visible: true,
	};
};
