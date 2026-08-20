import { describe, expect, it } from "vitest";

import {
	NewThreadSentences,
	NewThreadSentenceWords,
	PickNewThreadSentence,
	ProjectMarker,
	SentenceWordDelay,
} from "../../modules/frontend/src/lib/root/new-thread-sentence";

describe("new thread sentence", () => {
	/**
	 * The project is the one word in the line that can be pressed. A variant
	 * naming it twice would render two selectors that disagree; one naming it
	 * never would leave the surface with no way to choose a project at all.
	 */
	it("names the project exactly once in every variant", () => {
		expect(NewThreadSentences.length).toBeGreaterThan(1);
		for (const sentence of NewThreadSentences) {
			expect(sentence.split(ProjectMarker).length - 1, sentence).toBe(1);
		}
	});

	it("cuts the line at words and keeps punctuation glued to the project", () => {
		const words = NewThreadSentenceWords(`A new thread in ${ProjectMarker}.`);

		expect(words.map((word) => word.text)).toEqual(["A", "new", "thread", "in", ""]);
		expect(words.at(-1)).toEqual({
			delay_ms: SentenceWordDelay(4),
			leading_space: true,
			prefix: "",
			project: true,
			suffix: ".",
			text: "",
		});
	});

	it("carries the project's own leading text when a variant opens on it", () => {
		const words = NewThreadSentenceWords(`${ProjectMarker} is open. What first?`);

		expect(words[0]).toEqual({
			delay_ms: 0,
			leading_space: false,
			prefix: "",
			project: true,
			suffix: "",
			text: "",
		});
		expect(words.map((word) => word.project)).toEqual([true, false, false, false, false]);
		expect(
			words
				.map(
					(word) =>
						`${word.leading_space ? " " : ""}${word.prefix}${word.project ? "artisan-editor" : word.text}${word.suffix}`,
				)
				.join(""),
		).toBe("artisan-editor is open. What first?");
	});

	/** The stagger reads as one motion, and stops growing before it reads as a queue. */
	it("staggers the reveal and caps it", () => {
		expect(SentenceWordDelay(0)).toBe(0);
		expect(SentenceWordDelay(1)).toBe(45);
		expect(SentenceWordDelay(10)).toBe(450);
		expect(SentenceWordDelay(40)).toBe(SentenceWordDelay(10));
		expect(SentenceWordDelay(-3)).toBe(0);
	});

	it("never repeats the line it just showed", () => {
		const vocabulary = ["first ${marker}", "second ${marker}"];

		expect(PickNewThreadSentence(undefined, () => 0, vocabulary)).toBe(vocabulary[0]);
		expect(PickNewThreadSentence(vocabulary[0], () => 0, vocabulary)).toBe(vocabulary[1]);
		expect(PickNewThreadSentence(vocabulary[1], () => 0.99, vocabulary)).toBe(vocabulary[0]);
		/** A random that overshoots its own range must not fall off the list. */
		expect(PickNewThreadSentence(undefined, () => 1, vocabulary)).toBe(vocabulary[1]);
		expect(PickNewThreadSentence(undefined, () => -1, vocabulary)).toBe(vocabulary[0]);
	});
});
