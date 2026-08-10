import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Exit } from "effect";

import {
	MakeCodexUsage,
	map_codex_rate_limits_to_quota_windows,
	type CodexRateLimitsReadResult,
} from "../../modules/engines/src/codex/usage";
import {
	CodexProcessFactory,
	CodexProcessFactoryLive,
} from "../../modules/engines/src/codex/process";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const original_scenario = process.env.FAKE_APP_SERVER_SCENARIO;

afterEach(() => {
	if (original_scenario === undefined) delete process.env.FAKE_APP_SERVER_SCENARIO;
	else process.env.FAKE_APP_SERVER_SCENARIO = original_scenario;
});

const MakeUsage = (factory: typeof CodexProcessFactory.Service) =>
	MakeCodexUsage({
		executable: process.execPath,
		executable_args: [fixture_path],
		factory,
	});

const Usage = () =>
	Effect.gen(function* () {
		const factory = yield* CodexProcessFactory;

		return yield* MakeUsage(factory);
	});

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

describe("MakeCodexUsage ACP account usage", () => {
	it("does no work until execution, then asks the configured app-server without a filesystem service", async () => {
		let spawns = 0;
		const factory: typeof CodexProcessFactory.Service = {
			Spawn: () =>
				Effect.sync(() => {
					spawns += 1;
				}).pipe(Effect.andThen(Effect.die("stop after proving execution is lazy"))),
		};
		const usage = MakeUsage(factory);

		expect(spawns).toBe(0);
		await Effect.runPromise(Effect.exit(usage));
		expect(spawns).toBe(1);
	});

	it("reads authenticated account and rate limits through app-server with no auth file", async () => {
		const usage = await Effect.runPromise(
			Usage().pipe(Effect.provide(CodexProcessFactoryLive)),
		);

		expect(usage).toEqual({
			account_email: "fake@example.com",
			authentication: { state: "authenticated" },
			windows: [
				{
					id: "codex:primary",
					kind: "session",
					label: "Codex",
					percent_used: 25,
					resets_at: new Date(1_800_000_000 * 1_000).toISOString(),
					window_minutes: 300,
				},
			],
		});
	});

	it("maps ACP account and login responses to unauthenticated usage", async () => {
		process.env.FAKE_APP_SERVER_SCENARIO = "usage-account-unauthenticated";
		const account = await Effect.runPromise(
			Usage().pipe(Effect.provide(CodexProcessFactoryLive)),
		);
		expect(account).toMatchObject({
			authentication: { reason: "OpenAI authentication required", state: "unauthenticated" },
			windows: [],
		});

		process.env.FAKE_APP_SERVER_SCENARIO = "usage-login-required";
		const login = await Effect.runPromise(
			Usage().pipe(Effect.provide(CodexProcessFactoryLive)),
		);
		expect(login).toMatchObject({
			authentication: { reason: "Not logged in to Codex", state: "unauthenticated" },
			windows: [],
		});
	});

	it.each(["usage-rate-limit-failure", "usage-rate-limit-malformed"])(
		"fails truthfully for %s ACP failures",
		async (scenario) => {
			process.env.FAKE_APP_SERVER_SCENARIO = scenario;
			const result = await Effect.runPromise(
				Effect.exit(Usage().pipe(Effect.provide(CodexProcessFactoryLive))),
			);

			expect(Exit.isFailure(result)).toBe(true);
			expect(JSON.stringify(result)).toContain("EngineProtocolError");
		},
	);
});
