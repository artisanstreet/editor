import { Buffer } from "node:buffer";

import { Cause, DateTime, Duration, Effect, Option, Schema } from "effect";

import {
	type EngineAccountUsage,
	type EngineFailure,
	type EngineQuotaWindow,
	EngineProcessError,
	EngineProtocolError,
	EngineUnavailableError,
} from "../engine";
import { EngineProcessFactory } from "../process/process";

/** Configures the external Claude CLI invocation used to read account usage. @since 0.6.0 */
export interface ClaudeUsageOptions {
	/** Matches `ClaudeEngineOptions.executable`; defaults to `"claude"`. */
	readonly executable?: string;
	/** Matches `ClaudeEngineOptions.executable_args`; defaults to `[]`. */
	readonly executable_args?: ReadonlyArray<string>;
	/** Owns the only provider process this operation may start. */
	readonly factory?: typeof EngineProcessFactory.Service;
	/** Testable upper bound for a CLI that never settles. */
	readonly timeout?: Duration.Input;
}

const claude_cli_usage_args = ["-p", "/usage", "--output-format", "json"] as const;
const claude_cli_usage_max_bytes = 1_048_576;
const claude_cli_usage_timeout = "20 seconds";

/** Decodes the `result` field of `claude -p "/usage" --output-format json`. */
const ClaudeCliUsageResultSchema = Schema.Struct({ result: Schema.String });

function clamp_percent(value: number): number {
	return Math.min(100, Math.max(0, value));
}

/** Matches `Current session: N% used` in the CLI's `/usage` result text. */
const CLAUDE_CLI_SESSION_LINE = /^Current session:\s*(\d+)%\s*used\b/;
/** Matches `Current week (all models): N% used`; checked before the labeled pattern below. */
const CLAUDE_CLI_WEEKLY_ALL_LINE = /^Current week \(all models\):\s*(\d+)%\s*used\b/;
/** Matches `Current week (<Label>): N% used` for any other per-model weekly bucket. */
const CLAUDE_CLI_WEEKLY_LABELED_LINE = /^Current week \(([^)]+)\):\s*(\d+)%\s*used\b/;
/** Captures the English wall-clock reset clause emitted by Claude Code. */
const CLAUDE_CLI_RESET_CLAUSE =
	/\bresets\s+([a-z]+)\s+(\d{1,2}),\s+(\d{1,2})(?::(\d{2}))?\s*(am|pm)\s*\(([^)]+)\)\s*$/i;
const CLAUDE_CLI_MONTHS: Readonly<Record<string, number>> = {
	apr: 4,
	aug: 8,
	dec: 12,
	feb: 2,
	jan: 1,
	jul: 7,
	jun: 6,
	mar: 3,
	may: 5,
	nov: 11,
	oct: 10,
	sep: 9,
};

/** Turns a provider-supplied label into a stable, lowercase, hyphenated id fragment. */
function slugify_claude_cli_label(label: string) {
	return label
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-+|-+$/g, "");
}

/** Converts Claude's wall-clock reset clause to a real instant through Effect's IANA-zone support. */
function parse_claude_cli_reset_at(line: string, at_ms: number): string | undefined {
	const matched = CLAUDE_CLI_RESET_CLAUSE.exec(line);
	const month_name = matched?.at(1);
	const day_text = matched?.at(2);
	const hour_text = matched?.at(3);
	const minute_text = matched?.at(4);
	const meridiem = matched?.at(5)?.toLowerCase();
	const time_zone = matched?.at(6);
	if (
		month_name === undefined ||
		day_text === undefined ||
		hour_text === undefined ||
		meridiem === undefined ||
		time_zone === undefined
	)
		return undefined;

	const month = CLAUDE_CLI_MONTHS[month_name.slice(0, 3).toLowerCase()];
	const day = Number(day_text);
	const twelve_hour = Number(hour_text);
	const minute = Number(minute_text ?? "0");
	if (
		month === undefined ||
		day < 1 ||
		day > 31 ||
		twelve_hour < 1 ||
		twelve_hour > 12 ||
		minute < 0 ||
		minute > 59
	)
		return undefined;

	const current = DateTime.makeZoned(at_ms, { timeZone: time_zone });
	if (Option.isNone(current)) return undefined;
	const current_parts = DateTime.toParts(current.value);
	const year = current_parts.year + (current_parts.month === 12 && month === 1 ? 1 : 0);
	const hour = (twelve_hour % 12) + (meridiem === "pm" ? 12 : 0);
	const reset = DateTime.makeZoned(
		{ day, hour, millisecond: 0, minute, month, second: 0, year },
		{ adjustForTimeZone: true, timeZone: time_zone },
	);
	if (Option.isNone(reset)) return undefined;
	const reset_parts = DateTime.toParts(reset.value);
	if (
		reset_parts.year !== year ||
		reset_parts.month !== month ||
		reset_parts.day !== day ||
		reset_parts.hour !== hour ||
		reset_parts.minute !== minute
	)
		return undefined;

	return new Date(DateTime.toEpochMillis(reset.value)).toISOString();
}

/** Parses provider-owned CLI text without retaining credentials or raw account data. */
export function parse_claude_cli_usage_windows(
	result_text: string,
	at_ms = Date.now(),
): ReadonlyArray<EngineQuotaWindow> {
	const windows: Array<EngineQuotaWindow> = [];
	const seen = new Set<string>();
	const push_window = (window: EngineQuotaWindow) => {
		if (seen.has(window.id)) return;
		seen.add(window.id);
		windows.push(window);
	};

	for (const raw_line of result_text.split("\n")) {
		const line = raw_line.trim();
		const session_percent = CLAUDE_CLI_SESSION_LINE.exec(line)?.at(1);
		if (session_percent !== undefined) {
			const resets_at = parse_claude_cli_reset_at(line, at_ms);
			push_window({
				id: "five_hour",
				kind: "session",
				percent_used: clamp_percent(Number(session_percent)),
				...(resets_at === undefined ? {} : { resets_at }),
				window_minutes: 300,
			});
			continue;
		}
		const weekly_all_percent = CLAUDE_CLI_WEEKLY_ALL_LINE.exec(line)?.at(1);
		if (weekly_all_percent !== undefined) {
			const resets_at = parse_claude_cli_reset_at(line, at_ms);
			push_window({
				id: "seven_day",
				kind: "weekly",
				percent_used: clamp_percent(Number(weekly_all_percent)),
				...(resets_at === undefined ? {} : { resets_at }),
				window_minutes: 10_080,
			});
			continue;
		}
		const weekly_labeled = CLAUDE_CLI_WEEKLY_LABELED_LINE.exec(line);
		const label = weekly_labeled?.at(1);
		const weekly_labeled_percent = weekly_labeled?.at(2);
		if (label !== undefined && weekly_labeled_percent !== undefined) {
			const resets_at = parse_claude_cli_reset_at(line, at_ms);
			push_window({
				id: `seven_day:${slugify_claude_cli_label(label)}`,
				kind: "weekly",
				label,
				percent_used: clamp_percent(Number(weekly_labeled_percent)),
				...(resets_at === undefined ? {} : { resets_at }),
				window_minutes: 10_080,
			});
		}
	}

	return windows;
}

const ReadBounded = (stream: AsyncIterable<Uint8Array>, maximum: number) =>
	Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.byteLength;
				if (total > maximum)
					throw new Error(`Claude CLI usage output exceeded ${maximum} bytes`);
				chunks.push(chunk);
			}
			return Buffer.concat(chunks);
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "read" }),
	});

/**
 * Reads quota windows exclusively through the configured external `claude`
 * executable. Constructing this Effect performs no filesystem, credential,
 * network, or provider process access.
 */
export function MakeClaudeUsage(
	options: ClaudeUsageOptions = {},
): Effect.Effect<EngineAccountUsage, EngineFailure> {
	const factory = options.factory;
	if (factory === undefined)
		return Effect.fail(
			new EngineUnavailableError({
				engine_id: "claude",
				message: "Claude CLI usage has no process factory configured",
			}),
		);

	return Effect.scoped(
		Effect.gen(function* () {
			const handle = yield* factory.Spawn({
				args: [...(options.executable_args ?? []), ...claude_cli_usage_args],
				command: options.executable ?? "claude",
			});
			const [stdout, , exit] = yield* Effect.all(
				[
					ReadBounded(handle.Stdout, claude_cli_usage_max_bytes),
					ReadBounded(handle.Stderr, claude_cli_usage_max_bytes),
					handle.Exit,
				],
				{ concurrency: "unbounded" },
			).pipe(Effect.ensuring(handle.Close));
			if (exit.code !== 0 || exit.signal !== null)
				return yield* Effect.fail(
					new EngineUnavailableError({
						engine_id: "claude",
						message: `Claude CLI usage exited with code ${String(exit.code)} and signal ${String(exit.signal)}`,
					}),
				);
			const response = yield* Schema.decodeUnknownEffect(
				Schema.fromJsonString(ClaudeCliUsageResultSchema),
			)(new TextDecoder().decode(stdout)).pipe(
				Effect.mapError(
					() =>
						new EngineProtocolError({
							engine_id: "claude",
							message: "Claude CLI usage did not return valid JSON",
						}),
				),
			);
			const windows = parse_claude_cli_usage_windows(response.result);
			if (windows.length === 0)
				return yield* Effect.fail(
					new EngineUnavailableError({
						engine_id: "claude",
						message: "Claude CLI usage reported no usage windows",
					}),
				);
			return {
				authentication: { state: "authenticated" },
				windows,
			} satisfies EngineAccountUsage;
		}),
	).pipe(
		Effect.timeout(options.timeout ?? claude_cli_usage_timeout),
		Effect.mapError((cause: EngineFailure | Cause.TimeoutError) =>
			cause._tag === "EngineProcessError" ||
			cause._tag === "EngineProtocolError" ||
			cause._tag === "EngineUnavailableError"
				? cause
				: new EngineUnavailableError({
						engine_id: "claude",
						message: "Claude CLI usage timed out",
					}),
		),
	);
}
