import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";

import {
	cursor_auth_file_path,
	MakeCursorUsage,
	map_cursor_period_usage_to_quota_windows,
} from "@artisan/engines";

const period = {
	billingCycleEnd: "1789400994214",
	billingCycleStart: "1786722594214",
	displayMessage: "You've used 0% of your included usage",
	planUsage: {
		apiPercentUsed: 40,
		autoPercentUsed: 15,
		bonusSpend: 3,
		totalPercentUsed: 1.5,
		totalSpend: 3,
	},
	spendLimitUsage: {
		overallLimit: 1_000,
		overallRemaining: 750,
	},
	autoBucketModels: ["auto", "composer-2.5"],
};

describe("Cursor account usage", () => {
	it("maps both plan pools and configured on-demand usage", () => {
		expect(map_cursor_period_usage_to_quota_windows(period)).toEqual([
			{
				id: "cursor:cursor-models",
				kind: "monthly",
				label: "Cursor models",
				percent_used: 15,
				resets_at: "2026-09-14T15:49:54.214Z",
				scope: "shared",
				window_minutes: 44_640,
			},
			{
				id: "cursor:other-models",
				kind: "monthly",
				label: "Other models",
				percent_used: 40,
				resets_at: "2026-09-14T15:49:54.214Z",
				scope: "shared",
				window_minutes: 44_640,
			},
			{
				id: "cursor:on-demand",
				kind: "monthly",
				label: "On-demand",
				percent_used: 25,
				resets_at: "2026-09-14T15:49:54.214Z",
				scope: "shared",
				window_minutes: 44_640,
			},
		]);
	});

	it("retains the aggregate fallback for legacy dashboard responses", () => {
		expect(
			map_cursor_period_usage_to_quota_windows({
				billingCycleEnd: period.billingCycleEnd,
				billingCycleStart: period.billingCycleStart,
				displayMessage: period.displayMessage,
				planUsage: {
					bonusSpend: 3,
					totalPercentUsed: 1.5,
					totalSpend: 3,
				},
			}),
		).toEqual([
			{
				id: "cursor:included-usage",
				kind: "monthly",
				label: "Included usage",
				percent_used: 1.5,
				resets_at: "2026-09-14T15:49:54.214Z",
				scope: "shared",
				window_minutes: 44_640,
			},
		]);
	});

	it.effect("fetches usage with the Cursor credential and marks the surface supported", () => {
		let authorization: string | undefined;
		return Effect.gen(function* () {
			const usage = yield* MakeCursorUsage({
				Fetch: async (_url, init) => {
					authorization = init.headers.Authorization;
					return { json: async () => period, ok: true, status: 200 };
				},
				ReadAccessToken: async () => "private-token",
			});

			expect(authorization).toBe("Bearer private-token");
			expect(usage).toMatchObject({
				authentication: { state: "authenticated" },
				quota_surface: "supported",
			});
			expect(usage.windows).toHaveLength(3);
		});
	});

	it.effect("reports signed-out without calling the dashboard", () => {
		let called = false;
		return Effect.gen(function* () {
			const usage = yield* MakeCursorUsage({
				Fetch: async () => {
					called = true;
					return { json: async () => period, ok: true, status: 200 };
				},
				ReadAccessToken: async () => undefined,
			});

			expect(called).toBe(false);
			expect(usage).toEqual({
				authentication: {
					reason: "Sign in to Cursor from Settings.",
					state: "unauthenticated",
				},
				quota_surface: "supported",
				windows: [],
			});
		});
	});

	it("resolves Cursor's Windows credential location", () => {
		expect(
			cursor_auth_file_path({
				environment: { APPDATA: "C:\\Users\\Ada\\AppData\\Roaming" },
				home_directory: "C:\\Users\\Ada",
				platform: "win32",
			}),
		).toBe("C:\\Users\\Ada\\AppData\\Roaming\\Cursor\\auth.json");
	});
});
