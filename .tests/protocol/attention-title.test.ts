import { describe, expect, it } from "vitest";

import {
	AttentionCountFromTitle,
	AttentionTitleMarkerFor,
	forge_repair_title_marker,
	TitleRequestsForgeRepair,
	TitleSignalsAwaitingAnswer,
} from "@artisan/protocol";

describe("attention title channel", () => {
	it("round-trips the count through a marked title", () => {
		expect(
			AttentionCountFromTitle(`${AttentionTitleMarkerFor(2)} Thread › Artisan Editor`),
		).toBe(2);
		expect(AttentionCountFromTitle(`[Dev] ${AttentionTitleMarkerFor(0)} Artisan Editor`)).toBe(
			0,
		);
		expect(AttentionCountFromTitle(`${AttentionTitleMarkerFor(9999)} anywhere`)).toBe(9999);
	});

	it("reads no marker from ordinary titles, including parenthesised thread names", () => {
		expect(AttentionCountFromTitle("Thread › Artisan Editor")).toBeUndefined();
		expect(AttentionCountFromTitle("(3) fix the build › Artisan Editor")).toBeUndefined();
		expect(AttentionCountFromTitle("")).toBeUndefined();
	});

	it("publishes whole non-negative counts only", () => {
		expect(AttentionTitleMarkerFor(2.9)).toBe("(2)\u2060");
		expect(AttentionTitleMarkerFor(-4)).toBe("(0)\u2060");
	});

	/**
	 * The awaiting-answer flag rides the same marker: `(3?)` beside a count,
	 * `(?)` alone when the question is the only thing waiting, and the count
	 * stays readable either way.
	 */
	it("carries the awaiting-answer flag without disturbing the count", () => {
		expect(AttentionTitleMarkerFor(3, true)).toBe("(3?)\u2060");
		expect(AttentionTitleMarkerFor(0, true)).toBe("(?)\u2060");
		expect(AttentionTitleMarkerFor(0, false)).toBe("(0)\u2060");

		const flagged = `${AttentionTitleMarkerFor(3, true)} Thread \u203a Artisan Editor`;
		expect(AttentionCountFromTitle(flagged)).toBe(3);
		expect(TitleSignalsAwaitingAnswer(flagged)).toBe(true);

		const lone = `${AttentionTitleMarkerFor(0, true)} Thread \u203a Artisan Editor`;
		expect(AttentionCountFromTitle(lone)).toBe(0);
		expect(TitleSignalsAwaitingAnswer(lone)).toBe(true);

		expect(TitleSignalsAwaitingAnswer(`${AttentionTitleMarkerFor(2)} anywhere`)).toBe(false);
		/** A thread name's bare question mark carries no joiner and cannot forge the flag. */
		expect(TitleSignalsAwaitingAnswer("(?) what now \u203a Artisan Editor")).toBe(false);
		expect(AttentionCountFromTitle("(?) what now \u203a Artisan Editor")).toBeUndefined();
	});

	/**
	 * The repair ask shares the title with the attention count, so the two must
	 * be readable independently \u2014 and no thread name may forge either.
	 */
	it("carries the forge repair ask without disturbing the count", () => {
		const asking = `${AttentionTitleMarkerFor(3)} Thread \u203a Artisan Editor${forge_repair_title_marker}`;

		expect(TitleRequestsForgeRepair(asking)).toBe(true);
		expect(AttentionCountFromTitle(asking)).toBe(3);
	});

	it("cannot be forged by a thread name or an ordinary marked title", () => {
		expect(TitleRequestsForgeRepair("Thread \u203a Artisan Editor")).toBe(false);
		expect(TitleRequestsForgeRepair(`${AttentionTitleMarkerFor(2)} reconnect forge`)).toBe(
			false,
		);
		expect(TitleRequestsForgeRepair("")).toBe(false);
	});
});
