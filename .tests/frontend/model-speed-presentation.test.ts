import { describe, expect, it } from "vitest";

import { speed_option_presentation } from "../../modules/frontend/src/lib/engine/speed-presentation";

describe("model speed presentation", () => {
	it("renders Fast in a gold tone", () => {
		expect(speed_option_presentation({ id: "fast", label: "Provider fast" })).toEqual({
			class_name: "text-amber-600 dark:text-amber-400",
			label: "Fast",
		});
	});

	it("reserves Superfast and its purple-to-green gradient for Cerebras GPT tiers", () => {
		expect(
			speed_option_presentation({ id: "superfast", label: "Provider accelerated" }),
		).toEqual({
			class_name: "bg-linear-to-r from-purple-500 to-green-500 bg-clip-text text-transparent",
			label: "Superfast",
		});
	});

	it("preserves labels for ordinary and future unrelated tiers", () => {
		expect(speed_option_presentation({ id: "economy", label: "Economy" })).toEqual({
			class_name: "",
			label: "Economy",
		});
	});
});
