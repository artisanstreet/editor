import { describe, expect, it } from "vitest";

import {
	ComposerPlaceholders,
	MakeComposerPlaceholderState,
	PickComposerPlaceholder,
	UpdateComposerPlaceholderState,
} from "../../modules/frontend/src/lib/composer-placeholder";

describe("composer placeholder lifecycle", () => {
	it("loads one hundred unique curated phrases", () => {
		expect(ComposerPlaceholders).toHaveLength(100);
		expect(new Set(ComposerPlaceholders).size).toBe(100);
	});

	it("selects a visible initial phrase", () => {
		const state = MakeComposerPlaceholderState(() => 0);
		expect(state).toEqual({ generation: 0, phrase: ComposerPlaceholders[0], visible: true });
	});

	it("refreshes only after the placeholder leaves and returns", () => {
		const initial = MakeComposerPlaceholderState(() => 0);
		const still_empty = UpdateComposerPlaceholderState(initial, "", () => 0.5);
		const hidden = UpdateComposerPlaceholderState(still_empty, "a", () => 0.5);
		const still_hidden = UpdateComposerPlaceholderState(hidden, "ab", () => 0.5);
		const visible_again = UpdateComposerPlaceholderState(still_hidden, "", () => 0);

		expect(still_empty).toBe(initial);
		expect(still_hidden).toBe(hidden);
		expect(hidden.phrase).toBe(initial.phrase);
		expect(visible_again.visible).toBe(true);
		expect(visible_again.generation).toBe(1);
		expect(visible_again.phrase).not.toBe(initial.phrase);
	});

	it("avoids immediate repeats without retrying randomness", () => {
		const vocabulary = ["First", "Second", "Third"];
		expect(PickComposerPlaceholder("First", () => 0, vocabulary)).toBe("Second");
		expect(PickComposerPlaceholder("Second", () => 0.4, vocabulary)).toBe("Third");
	});

	it("keeps a stable fallback for an empty vocabulary", () => {
		expect(PickComposerPlaceholder(undefined, () => 0, [])).toBe("Do anything");
	});
});
