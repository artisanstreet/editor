import { describe, expect, it } from "vitest";

import { make_desktop_window_activity } from "@artisan/desktop";

describe("desktop native activity indicator", () => {
	it("uses an indeterminate native progress signal only for state transitions", () => {
		const calls: Array<readonly [number, { readonly mode: "indeterminate" } | undefined]> = [];
		const activity = make_desktop_window_activity({
			setProgressBar: (progress, options) => calls.push([progress, options]),
		});

		expect(activity.SetWorking(true)).toBe(true);
		expect(activity.SetWorking(true)).toBe(false);
		expect(activity.RestoreIdle()).toBe(true);
		expect(activity.RestoreIdle()).toBe(false);
		expect(calls).toEqual([
			[2, { mode: "indeterminate" }],
			[-1, undefined],
		]);
	});

	it("keeps the shell usable when native progress is unsupported", () => {
		const activity = make_desktop_window_activity({
			setProgressBar: () => {
				throw new Error("unsupported");
			},
		});

		expect(activity.SetWorking(true)).toBe(true);
		expect(activity.RestoreIdle()).toBe(true);
	});
});
