import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { make_desktop_window_activity } from "@artisan/desktop";

describe("desktop native activity indicator", () => {
	it("uses an indeterminate native progress signal only for state transitions", () => {
		const calls: Array<readonly [number, { readonly mode: "indeterminate" } | undefined]> = [];
		const activity = Effect.runSync(
			make_desktop_window_activity({
				setProgressBar: (progress, options) => calls.push([progress, options]),
			}),
		);

		expect(Effect.runSync(activity.SetWorking(true))).toBe(true);
		expect(Effect.runSync(activity.SetWorking(true))).toBe(false);
		expect(Effect.runSync(activity.RestoreIdle)).toBe(true);
		expect(Effect.runSync(activity.RestoreIdle)).toBe(false);
		expect(calls).toEqual([
			[2, { mode: "indeterminate" }],
			[-1, undefined],
		]);
	});

	it("keeps the shell usable when native progress is unsupported", () => {
		const activity = Effect.runSync(
			make_desktop_window_activity({
				setProgressBar: () => {
					throw new Error("unsupported");
				},
			}),
		);

		expect(Effect.runSync(activity.SetWorking(true))).toBe(true);
		expect(Effect.runSync(activity.RestoreIdle)).toBe(true);
	});
});
