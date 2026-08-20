/**
 * The line a new thread opens with.
 *
 * One sentence, in which the project is a word you can change. That is the
 * whole of the idea: choosing where the work happens should read like part of
 * saying what the work is, rather than a control bolted above the composer.
 *
 * The sentence arrives out of focus and sharpens word by word, so the eye is
 * carried along it to the one word that is underlined — the only word in it
 * that is clickable.
 */

/** The place the project's own name takes in a sentence. */
export const ProjectMarker = "{project}";

/**
 * The sentences, all saying the same thing differently.
 *
 * Kept short and plain: this line is read once per visit and then typed over,
 * so it has to be legible at a glance and never clever enough to reread. Each
 * names the project exactly once — the variation is in the grammar around it.
 */
export const NewThreadSentences: ReadonlyArray<string> = [
	`What are we building in ${ProjectMarker} today?`,
	`What should happen in ${ProjectMarker} next?`,
	`A new thread in ${ProjectMarker}.`,
	`Pick up ${ProjectMarker} where you left it.`,
	`Where would you like to start in ${ProjectMarker}?`,
	`${ProjectMarker} is open. What first?`,
];

const fallback_sentence = `A new thread in ${ProjectMarker}.`;

/**
 * Which sentence this visit gets.
 *
 * Never the one just shown, so arriving here twice in a row does not read as a
 * page that failed to change.
 */
export const PickNewThreadSentence = (
	previous: string | undefined,
	random: () => number = Math.random,
	vocabulary: ReadonlyArray<string> = NewThreadSentences,
): string => {
	const choices = vocabulary.length > 0 ? vocabulary : [fallback_sentence];
	const unit = Math.min(0.999_999, Math.max(0, random()));
	let index = Math.floor(unit * choices.length);
	if (choices.length > 1 && choices[index] === previous) index = (index + 1) % choices.length;
	return choices[index] ?? fallback_sentence;
};

/**
 * The stagger, capped.
 *
 * Words land 45ms apart — inside the range a reveal reads as one motion rather
 * than a queue — and the cap keeps the longest sentence from finishing half a
 * second after the shortest one did.
 */
const StaggerStep = 45;
const StaggeredWords = 10;

export const SentenceWordDelay = (index: number): number =>
	Math.min(Math.max(0, index), StaggeredWords) * StaggerStep;

export type NewThreadSentenceWord = {
	readonly delay_ms: number;
	/** A semantic separator before every word after the first. */
	readonly leading_space: boolean;
	/** Anything glued in front of the project's own name, such as an opening quote. */
	readonly prefix: string;
	readonly project: boolean;
	/** Punctuation glued behind it, which must never wrap onto a line of its own. */
	readonly suffix: string;
	/** The word itself, for every word that is not the project. */
	readonly text: string;
};

/**
 * The sentence cut into the spans that reveal it.
 *
 * Cut at words rather than characters: a word is the unit the eye actually
 * lands on, and a per-character blur across a whole sentence reads as a machine
 * typing rather than as a page arriving.
 *
 * The project keeps whatever is stuck to it — the full stop in "a new thread in
 * {project}." — inside its own word, so the punctuation can neither drift a
 * space away from the name nor wrap without it.
 */
export const NewThreadSentenceWords = (template: string): ReadonlyArray<NewThreadSentenceWord> =>
	template
		.split(/\s+/u)
		.filter((token) => token.length > 0)
		.map((token, index): NewThreadSentenceWord => {
			const delay_ms = SentenceWordDelay(index);
			const leading_space = index > 0;
			const marker_at = token.indexOf(ProjectMarker);
			if (marker_at === -1)
				return {
					delay_ms,
					leading_space,
					prefix: "",
					project: false,
					suffix: "",
					text: token,
				};

			return {
				delay_ms,
				leading_space,
				prefix: token.slice(0, marker_at),
				project: true,
				suffix: token.slice(marker_at + ProjectMarker.length),
				text: "",
			};
		});
