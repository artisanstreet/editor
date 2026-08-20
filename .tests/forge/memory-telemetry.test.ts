import { describe, expect, it } from "vitest";

import {
	NextMemoryReportMark,
	ReadForgeMemory,
	memory_report_floor_mb,
	memory_report_step_mb,
} from "../../modules/forge/src/memory-telemetry";

describe("forge memory telemetry", () => {
	it("stays quiet while the process is merely running", () => {
		expect(NextMemoryReportMark(memory_report_floor_mb - 1, 0)).toBeUndefined();
		expect(NextMemoryReportMark(0, 0)).toBeUndefined();
	});

	it("reports the first mark once past the floor", () => {
		expect(NextMemoryReportMark(memory_report_floor_mb, 0)).toBe(memory_report_floor_mb);
		expect(NextMemoryReportMark(900, 0)).toBe(900);
	});

	/**
	 * A climb is only legible if each step is recorded; reporting every sample
	 * would bury it, and reporting once would lose its shape and its rate.
	 */
	it("reports again only after another step of growth", () => {
		expect(NextMemoryReportMark(1000 + memory_report_step_mb - 1, 1000)).toBeUndefined();
		expect(NextMemoryReportMark(1000 + memory_report_step_mb, 1000)).toBe(
			1000 + memory_report_step_mb,
		);
	});

	/** A process that sheds memory has not discovered anything new to say. */
	it("never retreats, so shedding and reclimbing stays quiet", () => {
		expect(NextMemoryReportMark(200, 4096)).toBeUndefined();
		expect(NextMemoryReportMark(4096, 4096)).toBeUndefined();
	});

	/**
	 * The split is the whole point: Forge has reached eighteen gigabytes without
	 * V8's four-gigabyte heap ceiling ever being touched, so a report that only
	 * carried the heap would describe none of it.
	 */
	it("separates the JavaScript heap from what is held outside it", () => {
		const sample = ReadForgeMemory();

		for (const value of Object.values(sample)) {
			expect(Number.isFinite(value)).toBe(true);
			expect(value).toBeGreaterThanOrEqual(0);
		}
		expect(sample).toHaveProperty("heap_used_mb");
		expect(sample).toHaveProperty("external_mb");
		expect(sample).toHaveProperty("array_buffers_mb");
		expect(sample).toHaveProperty("rss_mb");
	});
});
