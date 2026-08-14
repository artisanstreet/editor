import { describe, expect, it } from "vitest";

import { assess_unsettled_work } from "../../modules/backend/src/host/wake-lock-policy";

const approval_grace_ms = 5 * 60_000;

describe("wake lock policy", () => {
	it("releases when no work is unsettled", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [], progressing_count: 0 },
			1_000,
			approval_grace_ms,
		);

		expect(assessment).toEqual({ held_count: 0, hold: false });
	});

	it("holds for progressing work with no expiry to revisit", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [], progressing_count: 3 },
			1_000,
			approval_grace_ms,
		);

		expect(assessment).toEqual({ held_count: 3, hold: true });
	});

	it("holds for a fresh approval and schedules its grace expiry", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [10_000], progressing_count: 0 },
			10_000 + 60_000,
			approval_grace_ms,
		);

		expect(assessment).toEqual({
			held_count: 1,
			hold: true,
			recheck_at_ms: 10_000 + approval_grace_ms,
		});
	});

	it("stops holding for an approval past its grace", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [10_000], progressing_count: 0 },
			10_000 + approval_grace_ms,
			approval_grace_ms,
		);

		expect(assessment).toEqual({ held_count: 0, hold: false });
	});

	it("schedules the earliest expiry among several graced approvals", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [40_000, 20_000, 30_000], progressing_count: 1 },
			50_000,
			approval_grace_ms,
		);

		expect(assessment).toEqual({
			held_count: 4,
			hold: true,
			recheck_at_ms: 20_000 + approval_grace_ms,
		});
	});

	it("counts progressing work even when every approval has aged out", () => {
		const assessment = assess_unsettled_work(
			{ approval_requested_at_ms: [0], progressing_count: 2 },
			approval_grace_ms * 2,
			approval_grace_ms,
		);

		expect(assessment).toEqual({ held_count: 2, hold: true });
	});
});
