import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineProcessFactory } from "../../modules/engines/src/process/process";
import {
	ClaudeUsageResponseSchema,
	MakeClaudeUsage,
	map_claude_account_usage,
	parse_claude_cli_usage_windows,
	type ClaudeUsageFetch,
} from "../../modules/engines/src/claude/usage";

const decode = (body: unknown) => Schema.decodeUnknownSync(ClaudeUsageResponseSchema)(body);

/** Builds a fetch stub whose response never touches the network. */
function stub_fetch(response: {
	readonly json?: () => Promise<unknown>;
	readonly ok: boolean;
	readonly status: number;
}): ClaudeUsageFetch {
	return () =>
		Promise.resolve({
			json: response.json ?? (() => Promise.resolve({})),
			ok: response.ok,
			status: response.status,
		});
}

/** Builds a fake `claude` CLI process factory whose `/usage` result text and exit code are fixed. */
function stub_claude_cli_factory(
	result_text: string,
	exit_code = 0,
): typeof EngineProcessFactory.Service {
	return {
		Spawn: () =>
			Effect.succeed({
				Close: Effect.void,
				EndInput: Effect.void,
				Exit: Effect.succeed({ code: exit_code, signal: null }),
				Kill: () => Effect.void,
				Stderr: (async function* () {
					/* no stderr output */
				})(),
				Stdout: (async function* () {
					yield new TextEncoder().encode(JSON.stringify({ result: result_text }));
				})(),
				Write: () => Effect.void,
			}),
	};
}

/** Fails the test if `Spawn` is ever called; proves a 401 never reaches the CLI fallback. */
const never_spawn_factory: typeof EngineProcessFactory.Service = {
	Spawn: () => Effect.die("usage must not spawn the CLI on a 401"),
};

async function write_claude_credentials(config_dir: string): Promise<void> {
	await writeFile(
		join(config_dir, ".credentials.json"),
		JSON.stringify({ claudeAiOauth: { accessToken: "test-token" } }),
		"utf8",
	);
}

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
		const dir = await mkdtemp(join(tmpdir(), "artisan-usage-"));
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

describe("parse_claude_cli_usage_windows", () => {
	const at_ms = Date.parse("2026-07-28T12:00:00.000Z");
	const sample_result = [
		"You are currently using your subscription to power your Claude Code usage",
		"",
		"Current session: 17% used · resets Jul 29, 7:50am (Europe/Oslo)",
		"Current week (all models): 3% used · resets Aug 5, 12am (Europe/Oslo)",
		"Current week (Fable): 5% used · resets Aug 5, 12am (Europe/Oslo)",
		"",
		"What's contributing to your limits usage?",
		"Last 24h · 2083 requests · 9 sessions",
		"  88% of your usage was at >150k context",
	].join("\n");

	it("maps the three recognized lines to windows, ignoring junk and behavioral lines", () => {
		expect(parse_claude_cli_usage_windows(sample_result, at_ms)).toEqual([
			{
				id: "five_hour",
				kind: "session",
				percent_used: 17,
				resets_at: "2026-07-29T05:50:00.000Z",
				window_minutes: 300,
			},
			{
				id: "seven_day",
				kind: "weekly",
				percent_used: 3,
				resets_at: "2026-08-04T22:00:00.000Z",
				window_minutes: 10_080,
			},
			{
				id: "seven_day:fable",
				kind: "weekly",
				label: "Fable",
				percent_used: 5,
				resets_at: "2026-08-04T22:00:00.000Z",
				window_minutes: 10_080,
			},
		]);
	});

	/**
	 * The CLI repeats a window's line across layouts. A repeated id is not
	 * cosmetic downstream — a keyed renderer throws on it and takes every
	 * engine section after this one down with it — so first sighting wins.
	 */
	it("reports each window id once when the CLI repeats its lines", () => {
		const repeated = parse_claude_cli_usage_windows(
			[
				"Current session: 17% used",
				"Current week (all models): 3% used",
				"Current session: 17% used",
				"Current week (all models): 4% used",
			].join("\n"),
			at_ms,
		);

		expect(repeated.map((window) => window.id)).toEqual(["five_hour", "seven_day"]);
		expect(repeated[1]?.percent_used).toBe(3);
	});

	it("omits reset timestamps when the clause is malformed or its IANA zone is invalid", () => {
		const invalid_zone = parse_claude_cli_usage_windows(
			"Current week (all models): 3% used · resets Aug 5, 12am (Not/AZone)",
			at_ms,
		);
		const impossible_date = parse_claude_cli_usage_windows(
			"Current week (all models): 3% used · resets Feb 30, 12am (Europe/Oslo)",
			at_ms,
		);

		expect(invalid_zone).toEqual([
			{ id: "seven_day", kind: "weekly", percent_used: 3, window_minutes: 10_080 },
		]);
		expect(impossible_date).toEqual([
			{ id: "seven_day", kind: "weekly", percent_used: 3, window_minutes: 10_080 },
		]);
	});

	it("resolves Dec-to-Jan and leap-year resets while rejecting DST-gap wall times", () => {
		const year_rollover = parse_claude_cli_usage_windows(
			"Current week (all models): 3% used · resets Jan 2, 12am (Europe/Oslo)",
			Date.parse("2026-12-29T12:00:00.000Z"),
		);
		const leap_day = parse_claude_cli_usage_windows(
			"Current week (all models): 3% used · resets Feb 29, 12am (Europe/Oslo)",
			Date.parse("2028-02-22T12:00:00.000Z"),
		);
		const dst_gap = parse_claude_cli_usage_windows(
			"Current week (all models): 3% used · resets Mar 29, 2:30am (Europe/Oslo)",
			Date.parse("2026-03-25T12:00:00.000Z"),
		);

		expect(year_rollover.at(0)?.resets_at).toBe("2027-01-01T23:00:00.000Z");
		expect(leap_day.at(0)?.resets_at).toBe("2028-02-28T23:00:00.000Z");
		expect(dst_gap.at(0)).not.toHaveProperty("resets_at");
	});

	it("clamps out-of-range percentages", () => {
		expect(parse_claude_cli_usage_windows("Current session: 142% used")).toEqual([
			{ id: "five_hour", kind: "session", percent_used: 100, window_minutes: 300 },
		]);
	});

	it("signals no windows when nothing recognizable is present", () => {
		expect(parse_claude_cli_usage_windows("Nothing here matches any known line.")).toEqual([]);
	});
});

describe("Claude usage CLI fallback", () => {
	let dir: string;

	afterEach(async () => {
		if (dir !== undefined) await rm(dir, { force: true, recursive: true });
	});

	it("falls back to the CLI on a 429 and reports its parsed windows as authenticated", async () => {
		dir = await mkdtemp(join(tmpdir(), "artisan-usage-"));
		await write_claude_credentials(dir);

		const usage = await Effect.runPromise(
			MakeClaudeUsage({
				claude_config_dir: dir,
				factory: stub_claude_cli_factory(
					"Current session: 17% used\n" + "Current week (all models): 3% used",
				),
				fetch: stub_fetch({ ok: false, status: 429 }),
			}),
		);

		expect(usage).toEqual({
			authentication: { state: "authenticated" },
			windows: [
				{ id: "five_hour", kind: "session", percent_used: 17, window_minutes: 300 },
				{ id: "seven_day", kind: "weekly", percent_used: 3, window_minutes: 10_080 },
			],
		});
	});

	it("fails with the original endpoint error when the CLI fallback also reports no windows", async () => {
		dir = await mkdtemp(join(tmpdir(), "artisan-usage-"));
		await write_claude_credentials(dir);

		const exit = await Effect.runPromiseExit(
			MakeClaudeUsage({
				claude_config_dir: dir,
				factory: stub_claude_cli_factory("Nothing usage-shaped in here."),
				fetch: stub_fetch({ ok: false, status: 429 }),
			}),
		);

		expect(exit._tag).toBe("Failure");
		const message = exit._tag === "Failure" ? JSON.stringify(exit.cause) : "";
		expect(message).toContain("429");
	});

	it("never spawns the CLI on a 401; reports unauthenticated instead", async () => {
		dir = await mkdtemp(join(tmpdir(), "artisan-usage-"));
		await write_claude_credentials(dir);

		const usage = await Effect.runPromise(
			MakeClaudeUsage({
				claude_config_dir: dir,
				factory: never_spawn_factory,
				fetch: stub_fetch({ ok: false, status: 401 }),
			}),
		);

		expect(usage).toEqual({
			authentication: { reason: "token expired or revoked", state: "unauthenticated" },
			windows: [],
		});
	});

	it("never spawns the CLI when credentials are missing", async () => {
		dir = await mkdtemp(join(tmpdir(), "artisan-usage-"));

		const usage = await Effect.runPromise(
			MakeClaudeUsage({ claude_config_dir: dir, factory: never_spawn_factory }),
		);

		expect(usage.authentication.state).toBe("unauthenticated");
	});
});
