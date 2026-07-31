import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { weekly_reset_duration } from "../../modules/frontend/src/lib/identity/weekly-reset";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar identity and thread rail regressions", () => {
	it("forks provider usage reads with the Effect 4 scoped concurrency API", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");

		expect(Effect.forkScoped).toBeTypeOf("function");
		expect(identity).toContain("const MakeUsageRequests = Queue.unbounded<UsageRequest>();");
		expect(identity).toContain("const usage_requests = yield* MakeUsageRequests;");
		expect(identity).not.toMatch(/yield\*\s*Queue\.unbounded<\{/);
		expect(identity).toContain(
			"Effect.forkScoped(FetchEngineUsage(request.engine_id, request.force))",
		);
		expect(identity).not.toMatch(/\bEffect\.fork\(/);
	});

	it("shows the latest trustworthy weekly reset on each shader-glass provider menu", () => {
		const identity = read("modules/frontend/src/routes/components/sidebar-identity.sv");
		const now = Date.parse("2026-07-31T12:00:00.000Z");

		expect(
			weekly_reset_duration(
				[
					{
						id: "session",
						kind: "session",
						percent_used: 10,
						resets_at: "2026-08-07T12:00:00.000Z",
					},
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{
						id: "model-weekly",
						kind: "weekly",
						percent_used: 30,
						resets_at: "2026-08-02T12:00:00.000Z",
					},
				],
				now,
			),
		).toBe("2 days");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T17:00:00.000Z",
					},
				],
				now,
			),
		).toBe("5 hours");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T12:45:00.000Z",
					},
				],
				now,
			),
		).toBe("45 minutes");
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-08-01T12:00:00.000Z",
					},
					{ id: "model-weekly", kind: "weekly", percent_used: 30 },
				],
				now,
			),
		).toBeUndefined();
		expect(
			weekly_reset_duration(
				[
					{
						id: "weekly",
						kind: "weekly",
						percent_used: 20,
						resets_at: "2026-07-31T11:00:00.000Z",
					},
				],
				now,
			),
		).toBeUndefined();

		expect(identity).toContain("weekly_reset_duration(engine.windows, checked_at_ms)");
		expect(identity).toContain('<ShaderGlassSurface class="w-full rounded-2xl">');
		expect(identity).toContain("bg-transparent! p-0! shadow-none! ring-0!");
		expect(identity).toContain('<DropdownMenuSeparator class="my-1" />');
		expect(identity).toContain(
			'resets in <span class="text-foreground">{weekly_reset}</span>.',
		);
	});

	it("keeps thread-list rows stationary while retaining proximity reveal", () => {
		const rail = read("modules/frontend/src/routes/components/thread-hover-rail.sv");
		const styles = read("modules/frontend/src/lib/styles/sidebar.css");

		expect(rail).toContain("<svelte:window onpointermove={TrackPointer} />");
		expect(rail).toContain('class="t-panel-slide-x');
		expect(rail).not.toContain("SetShifts");
		expect(rail).not.toContain("getComputedStyle");
		expect(rail).not.toContain("t-avatar");
		expect(styles).not.toContain("--avatar-");
		expect(styles).not.toContain(".t-avatar");
	});
});
