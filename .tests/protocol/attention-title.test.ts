import { describe, expect, it } from "vitest";

import { AttentionCountFromTitle, AttentionTitleMarkerFor } from "@artisan/protocol";

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
});
