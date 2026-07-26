import { describe, expect, it } from "vitest";
import { format_compact_diff_count } from "../../modules/frontend/src/lib/conversation/diff-stat";

describe("conversation diff stat", () => {
	it.each([
		[999, "999"],
		[1_000, "1k"],
		[3_100, "3.1k"],
		[10_000, "10k"],
		[999_999, "1000k"],
		[1_000_000, "1M"],
		[3_100_000, "3.1M"],
		[10_000_000, "10M"],
		[1_000_000_000, "1B"],
	])("formats %i inside the fixed-width diff column", (value, expected) => {
		expect(format_compact_diff_count(value)).toBe(expected);
	});
});
