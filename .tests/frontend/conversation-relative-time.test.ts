import { describe, expect, it } from "vitest";
import { format_relative_age } from "../../modules/frontend/src/lib/conversation/relative-time";

const now = Date.parse("2026-07-25T12:00:00.000Z");

describe("conversation relative time", () => {
	it.each([
		[23_000, "23s ago"],
		[60_000, "1m ago"],
		[3_600_000, "1h ago"],
		[86_400_000, "1d ago"],
		[604_800_000, "1w ago"],
	])("formats %i elapsed milliseconds", (elapsed, expected) => {
		expect(format_relative_age(now, new Date(now - elapsed).toISOString())).toBe(expected);
	});

	it("clamps future timestamps to the present", () => {
		expect(format_relative_age(now, new Date(now + 5_000).toISOString())).toBe("0s ago");
	});
});
