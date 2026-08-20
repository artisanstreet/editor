import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import type {
	EngineProcessFactory,
	EngineProcessSpawnInput,
} from "../../modules/engines/src/process/process";
import {
	MakeClaudeUsage,
	parse_claude_cli_usage_windows,
} from "../../modules/engines/src/claude/usage";

const bytes = (text: string) => new TextEncoder().encode(text);

const Stream = (chunks: ReadonlyArray<string>) =>
	(async function* () {
		for (const chunk of chunks) yield bytes(chunk);
	})();

interface FactoryOptions {
	readonly exit?: { readonly code: number | null; readonly signal: NodeJS.Signals | null };
	readonly stderr?: ReadonlyArray<string>;
	readonly stdout?: ReadonlyArray<string>;
	readonly waits?: boolean;
}

const MakeFactory = (options: FactoryOptions = {}) => {
	const spawns: Array<EngineProcessSpawnInput> = [];
	let closes = 0;
	let input_ends = 0;
	const factory: typeof EngineProcessFactory.Service = {
		Spawn: (input) => {
			spawns.push(input);
			return Effect.succeed({
				Close: Effect.sync(() => {
					closes += 1;
				}),
				EndInput: Effect.sync(() => {
					input_ends += 1;
				}),
				Exit: options.waits
					? Effect.never
					: Effect.succeed(options.exit ?? { code: 0, signal: null }),
				Kill: () => Effect.void,
				Stderr: Stream(options.stderr ?? []),
				Stdout: Stream(options.stdout ?? []),
				Write: () => Effect.void,
			});
		},
	};
	return { closes: () => closes, factory, input_ends: () => input_ends, spawns };
};

const ValidUsage = JSON.stringify({
	result:
		"Current session: 17% used · resets Jul 29, 7:50am (Europe/Oslo)\n" +
		"Current week (all models): 3% used · resets Aug 5, 12am (Europe/Oslo)",
});

describe("Claude CLI usage", () => {
	it("does no provider work at construction and spawns exactly once when usage is requested", async () => {
		const fixture = MakeFactory({ stdout: [ValidUsage] });
		const usage = MakeClaudeUsage({ factory: fixture.factory });

		expect(fixture.spawns).toEqual([]);
		await expect(Effect.runPromise(usage)).resolves.toMatchObject({
			authentication: { state: "authenticated" },
		});
		expect(fixture.spawns).toEqual([
			{ args: ["-p", "/usage", "--output-format", "json"], command: "claude" },
		]);
		expect(fixture.closes()).toBe(1);
	});

	/**
	 * `-p` takes its prompt from stdin. A pipe left open costs this read a flat
	 * three seconds — the CLI's grace period before it gives up waiting and
	 * proceeds ("no stdin data received in 3s") — spent against a usage deadline
	 * every engine refreshes against concurrently. The slash command is already
	 * in argv, so there is nothing to send and EOF goes immediately.
	 */
	it("closes stdin so the CLI never waits out its stdin grace period", async () => {
		const fixture = MakeFactory({ stdout: [ValidUsage] });

		await Effect.runPromise(MakeClaudeUsage({ factory: fixture.factory }));

		expect(fixture.input_ends()).toBe(1);
	});

	it("propagates the configured external executable and its fixed wrapper args", async () => {
		const fixture = MakeFactory({ stdout: [ValidUsage] });
		await Effect.runPromise(
			MakeClaudeUsage({
				executable: "C:\\Tools\\claude.cmd",
				executable_args: ["--profile", "work"],
				factory: fixture.factory,
			}),
		);

		expect(fixture.spawns).toEqual([
			{
				args: ["--profile", "work", "-p", "/usage", "--output-format", "json"],
				command: "C:\\Tools\\claude.cmd",
			},
		]);
	});

	it("parses deduplicated windows and IANA reset zones exclusively from CLI output", async () => {
		const fixture = MakeFactory({ stdout: [ValidUsage] });
		const usage = await Effect.runPromise(MakeClaudeUsage({ factory: fixture.factory }));

		expect(usage.windows).toEqual([
			{
				id: "five_hour",
				kind: "session",
				percent_used: 17,
				resets_at: "2026-07-29T05:50:00.000Z",
				scope: "shared",
				window_minutes: 300,
			},
			{
				id: "seven_day",
				kind: "weekly",
				percent_used: 3,
				resets_at: "2026-08-04T22:00:00.000Z",
				scope: "shared",
				window_minutes: 10_080,
			},
		]);
		expect(
			parse_claude_cli_usage_windows(
				"Current session: 17% used\nCurrent session: 99% used\n" +
					"Current week (Fable): 5% used · resets Aug 5, 12am (Europe/Oslo)",
				Date.parse("2026-07-28T12:00:00.000Z"),
			),
		).toEqual([
			{
				id: "five_hour",
				kind: "session",
				percent_used: 17,
				scope: "shared",
				window_minutes: 300,
			},
			{
				id: "seven_day:fable",
				kind: "weekly",
				label: "Fable",
				percent_used: 5,
				resets_at: "2026-08-04T22:00:00.000Z",
				scope: "model",
				window_minutes: 10_080,
			},
		]);
	});

	it.each([
		["nonzero", { exit: { code: 1, signal: null }, stdout: [ValidUsage] }],
		["malformed", { stdout: ["not-json"] }],
		["empty", { stdout: [JSON.stringify({ result: "nothing recognizable" })] }],
		["overflow", { stdout: ["x".repeat(1_048_577)] }],
	] as const)("returns a typed failure for %s CLI output", async (_name, options) => {
		const fixture = MakeFactory(options);
		const exit = await Effect.runPromiseExit(MakeClaudeUsage({ factory: fixture.factory }));

		expect(exit._tag).toBe("Failure");
		expect(JSON.stringify(exit)).toMatch(/Engine(?:Process|Protocol|Unavailable)Error/u);
		expect(fixture.closes()).toBe(1);
	});

	it("returns a typed timeout and closes the only spawned CLI", async () => {
		const fixture = MakeFactory({ waits: true });
		const exit = await Effect.runPromiseExit(
			MakeClaudeUsage({ factory: fixture.factory, timeout: "1 millis" }),
		);

		expect(exit._tag).toBe("Failure");
		expect(JSON.stringify(exit)).toContain("Claude CLI usage timed out");
		expect(fixture.closes()).toBe(1);
	});

	it("does not call a global HTTP implementation", async () => {
		const fixture = MakeFactory({ stdout: [ValidUsage] });
		const original_fetch = globalThis.fetch;
		globalThis.fetch = (() => Promise.reject(new Error("HTTP must not run"))) as typeof fetch;
		try {
			await Effect.runPromise(MakeClaudeUsage({ factory: fixture.factory }));
		} finally {
			globalThis.fetch = original_fetch;
		}
		expect(fixture.spawns).toHaveLength(1);
	});
});
