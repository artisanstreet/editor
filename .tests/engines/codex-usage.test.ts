import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Effect, FileSystem } from "effect";

import {
	MakeCodexUsage,
	map_codex_rate_limits_to_quota_windows,
	type CodexRateLimitsReadResult,
} from "../../modules/engines/src/codex/usage";
import { CodexProcessFactory } from "../../modules/engines/src/codex/process";

const never_spawn_factory: typeof CodexProcessFactory.Service = {
	Spawn: () => Effect.die("usage must not spawn Codex without saved credentials"),
};

describe("map_codex_rate_limits_to_quota_windows", () => {
	it("maps every bucket in rateLimitsByLimitId, one window per non-null slot", () => {
		const result: CodexRateLimitsReadResult = {
			rateLimits: undefined,
			rateLimitsByLimitId: {
				codex: {
					limitId: "codex",
					limitName: null,
					primary: {
						resetsAt: 1_800_000_000,
						usedPercent: 42.5,
						windowDurationMins: 300,
					},
					secondary: {
						resetsAt: 1_800_500_000,
						usedPercent: 10,
						windowDurationMins: 10_080,
					},
				},
				codex_bengalfox: {
					limitId: "codex_bengalfox",
					limitName: "GPT-5.3-Codex-Spark",
					primary: { resetsAt: null, usedPercent: 5, windowDurationMins: 43_200 },
					secondary: null,
				},
			},
		};

		expect(map_codex_rate_limits_to_quota_windows(result)).toEqual([
			{
				id: "codex:primary",
				kind: "session",
				percent_used: 42.5,
				resets_at: new Date(1_800_000_000 * 1_000).toISOString(),
				window_minutes: 300,
			},
			{
				id: "codex:secondary",
				kind: "weekly",
				percent_used: 10,
				resets_at: new Date(1_800_500_000 * 1_000).toISOString(),
				window_minutes: 10_080,
			},
			{
				id: "codex_bengalfox:primary",
				kind: "monthly",
				label: "GPT-5.3-Codex-Spark",
				percent_used: 5,
				window_minutes: 43_200,
			},
		]);
	});

	it("falls back to the single rateLimits snapshot when rateLimitsByLimitId is null", () => {
		const result: CodexRateLimitsReadResult = {
			rateLimits: {
				limitId: null,
				limitName: null,
				primary: { resetsAt: 1_700_000_000, usedPercent: 87, windowDurationMins: 10_080 },
				secondary: null,
			},
			rateLimitsByLimitId: null,
		};

		expect(map_codex_rate_limits_to_quota_windows(result)).toEqual([
			{
				id: "codex:primary",
				kind: "weekly",
				percent_used: 87,
				resets_at: new Date(1_700_000_000 * 1_000).toISOString(),
				window_minutes: 10_080,
			},
		]);
	});

	it("converts unix-seconds resetsAt to an ISO 8601 UTC string and clamps out-of-range percentages", () => {
		const result: CodexRateLimitsReadResult = {
			rateLimits: {
				limitId: "codex",
				limitName: null,
				primary: { resetsAt: 1_000, usedPercent: 150, windowDurationMins: undefined },
				secondary: { resetsAt: null, usedPercent: -20, windowDurationMins: 999 },
			},
			rateLimitsByLimitId: null,
		};
		const windows = map_codex_rate_limits_to_quota_windows(result);

		expect(windows).toEqual([
			{
				id: "codex:primary",
				kind: "unknown",
				percent_used: 100,
				resets_at: "1970-01-01T00:16:40.000Z",
			},
			{
				id: "codex:secondary",
				kind: "unknown",
				percent_used: 0,
				window_minutes: 999,
			},
		]);
		expect(windows[0]!.resets_at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?Z$/);
	});

	it("emits nothing when neither rateLimits nor rateLimitsByLimitId is present", () => {
		expect(map_codex_rate_limits_to_quota_windows({})).toEqual([]);
	});
});

describe("MakeCodexUsage authentication precheck", () => {
	let codex_home: string;

	beforeEach(async () => {
		codex_home = await mkdtemp(join(tmpdir(), "artisan-usage-"));
	});

	afterEach(async () => {
		await rm(codex_home, { force: true, recursive: true });
	});

	it("reports unauthenticated without spawning when auth.json is missing", async () => {
		const usage = await Effect.runPromise(
			Effect.gen(function* () {
				const file_system = yield* FileSystem.FileSystem;

				return yield* MakeCodexUsage({
					codex_home,
					executable: process.execPath,
					executable_args: ["--this-flag-must-never-run"],
					factory: never_spawn_factory,
					file_system,
				});
			}).pipe(Effect.provide(NodeFileSystem.layer)),
		);

		expect(usage).toEqual({
			authentication: {
				reason: "Codex CLI has no saved ChatGPT session or API key",
				state: "unauthenticated",
			},
			windows: [],
		});
	});

	it("reports unauthenticated without spawning when tokens and OPENAI_API_KEY are both null", async () => {
		await writeFile(
			join(codex_home, "auth.json"),
			JSON.stringify({
				OPENAI_API_KEY: null,
				auth_mode: "chatgpt",
				last_refresh: null,
				tokens: null,
			}),
			"utf8",
		);

		const usage = await Effect.runPromise(
			Effect.gen(function* () {
				const file_system = yield* FileSystem.FileSystem;

				return yield* MakeCodexUsage({
					codex_home,
					executable: process.execPath,
					executable_args: ["--this-flag-must-never-run"],
					factory: never_spawn_factory,
					file_system,
				});
			}).pipe(Effect.provide(NodeFileSystem.layer)),
		);

		expect(usage.authentication.state).toBe("unauthenticated");
		expect(usage.windows).toEqual([]);
	});
});
