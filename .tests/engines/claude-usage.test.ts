import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	ClaudeUsageResponseSchema,
	MakeClaudeUsage,
	map_claude_account_usage,
} from "../../modules/engines/src/claude/claude-usage";

const decode = (body: unknown) => Schema.decodeUnknownSync(ClaudeUsageResponseSchema)(body);

let created_dirs: Array<string> = [];

afterEach(async () => {
	await Promise.all(created_dirs.map((dir) => rm(dir, { recursive: true, force: true })));
	created_dirs = [];
});

describe("Claude usage mapping", () => {
	it("prefers limits[] and maps per-model labels, clamping percentages", () => {
		const usage = map_claude_account_usage(
			decode({
				five_hour: { utilization: 10, resets_at: "2026-07-29T00:00:00.000Z" },
				limits: [
					{
						kind: "session",
						percent: 42,
						resets_at: "2026-07-29T05:00:00.000Z",
						is_active: true,
					},
					{
						kind: "weekly_scoped",
						percent: 142,
						resets_at: "2026-08-01T00:00:00.000Z",
						scope: { model: { id: "claude-fable", display_name: "Fable" } },
					},
					{
						kind: "some_future_kind",
						percent: -5,
						resets_at: "2026-08-02T00:00:00.000Z",
					},
				],
			}),
		);

		expect(usage.authentication).toEqual({ state: "authenticated" });
		expect(usage.windows).toEqual([
			{
				id: "session",
				kind: "session",
				percent_used: 42,
				resets_at: "2026-07-29T05:00:00.000Z",
			},
			{
				id: "weekly_scoped:claude-fable",
				kind: "weekly",
				label: "Fable",
				percent_used: 100,
				resets_at: "2026-08-01T00:00:00.000Z",
			},
			{
				id: "some_future_kind",
				kind: "unknown",
				percent_used: 0,
				resets_at: "2026-08-02T00:00:00.000Z",
			},
		]);
	});

	it("falls back to five_hour/seven_day when limits[] is missing or empty", () => {
		const usage = map_claude_account_usage(
			decode({
				five_hour: { utilization: 20, resets_at: "2026-07-29T05:00:00.000Z" },
				seven_day: { utilization: 55, resets_at: "2026-08-01T00:00:00.000Z" },
				limits: [],
			}),
		);

		expect(usage.windows).toEqual([
			{
				id: "five_hour",
				kind: "session",
				percent_used: 20,
				resets_at: "2026-07-29T05:00:00.000Z",
				window_minutes: 300,
			},
			{
				id: "seven_day",
				kind: "weekly",
				percent_used: 55,
				resets_at: "2026-08-01T00:00:00.000Z",
				window_minutes: 10080,
			},
		]);
	});

	it("drops a malformed resets_at instead of failing", () => {
		const usage = map_claude_account_usage(
			decode({
				limits: [{ kind: "session", percent: 5, resets_at: "not-a-date" }],
			}),
		);

		expect(usage.windows).toEqual([{ id: "session", kind: "session", percent_used: 5 }]);
	});

	it("reports no windows when five_hour/seven_day are absent and limits[] is empty", () => {
		const usage = map_claude_account_usage(decode({}));

		expect(usage.windows).toEqual([]);
	});
});

describe("Claude usage credentials resolution", () => {
	it("reports unauthenticated, without a network call, when no credentials file exists", async () => {
		const dir = await mkdtemp(join(tmpdir(), "artisan-claude-usage-"));
		created_dirs.push(dir);

		const usage = await Effect.runPromise(MakeClaudeUsage({ claude_config_dir: dir }));

		expect(usage).toEqual({
			authentication: {
				reason: "No Claude Code subscription session was found",
				state: "unauthenticated",
			},
			windows: [],
		});
	});
});
