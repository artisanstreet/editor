import { describe, expect, it } from "vitest";

import {
	AttentionCountFromTitle,
	AttentionTitleMarkerFor,
	forge_repair_title_marker,
	TitleRequestsForgeRepair,
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
