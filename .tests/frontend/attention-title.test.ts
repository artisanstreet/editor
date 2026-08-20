import { describe, expect, it } from "vitest";

import { AttentionMarkedTitle } from "../../modules/frontend/src/lib/root/attention-title";

describe("attention marked titles", () => {
	it("adds, replaces, and removes the marker idempotently", () => {
		expect(AttentionMarkedTitle("Thread › Artisan Editor", 2)).toBe(
			"(2)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(2)\u2060 Thread › Artisan Editor", 2)).toBe(
			"(2)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(2)\u2060 Thread › Artisan Editor", 5)).toBe(
			"(5)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(2)\u2060 Thread › Artisan Editor", undefined)).toBe(
			"Thread › Artisan Editor",
		);
	});

	it("always yields the development marker the front of the title", () => {
		expect(AttentionMarkedTitle("[Dev] Thread › Artisan Editor", 1)).toBe(
			"[Dev] (1)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("[Dev] (1)\u2060 Thread › Artisan Editor", undefined)).toBe(
			"[Dev] Thread › Artisan Editor",
		);
	});

	it("never mistakes parenthesised digits belonging to the title for its own marker", () => {
		expect(AttentionMarkedTitle("(3) fix the build › Artisan Editor", undefined)).toBe(
			"(3) fix the build › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(3) fix the build › Artisan Editor", 2)).toBe(
			"(2)\u2060 (3) fix the build › Artisan Editor",
		);
	});

	it("rides the awaiting-answer flag on the same rewrite", () => {
		expect(AttentionMarkedTitle("Thread › Artisan Editor", 2, false, true)).toBe(
			"(2?)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(2?)\u2060 Thread › Artisan Editor", 0, false, true)).toBe(
			"(?)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(?)\u2060 Thread › Artisan Editor", 2, false, false)).toBe(
			"(2)\u2060 Thread › Artisan Editor",
		);
		expect(AttentionMarkedTitle("(2?)\u2060 Thread › Artisan Editor", undefined)).toBe(
			"Thread › Artisan Editor",
		);
	});
});
