import { describe, expect, it } from "vitest";

import {
	AttentionOverlayDescription,
	AttentionOverlayLabelFor,
	attention_overlay_labels,
	attention_overlay_sources,
} from "@artisan/desktop";

const png_signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

describe("attention overlay badge", () => {
	it("collapses every count into the ten drawable labels", () => {
		expect(AttentionOverlayLabelFor(1)).toBe("1");
		expect(AttentionOverlayLabelFor(9)).toBe("9");
		expect(AttentionOverlayLabelFor(10)).toBe("9+");
		expect(AttentionOverlayLabelFor(137)).toBe("9+");
	});

	it("describes the badge for assistive technology", () => {
		expect(AttentionOverlayDescription(1)).toBe("1 thread needs attention");
		expect(AttentionOverlayDescription(4)).toBe("4 threads need attention");
	});

	it("carries a real PNG at 16, 24, and 32 pixels for every label", () => {
		for (const label of attention_overlay_labels) {
			const source = attention_overlay_sources[label];
			const scales = [
				{ encoded: source.x1, width: 16 },
				{ encoded: source.x1_5, width: 24 },
				{ encoded: source.x2, width: 32 },
			];
			for (const { encoded, width } of scales) {
				const bytes = Buffer.from(encoded, "base64");
				expect(bytes.subarray(0, 8)).toEqual(png_signature);
				/** IHDR width lives at byte 16 of a PNG stream. */
				expect(bytes.readUInt32BE(16)).toBe(width);
			}
		}
	});
});
