import { Effect, Schema } from "effect";

import {
	type EngineAccountUsage,
	type EngineFailure,
	type EngineQuotaWindow,
	type EngineQuotaWindowKind,
	EngineProcessError,
	EngineProtocolError,
} from "../engine";
import { CodexAppServerResponseError } from "./protocol";
import { CodexAccountReadSchema } from "./probe";
import {
	open_codex_app_server_session,
	type CodexAppServerSessionFailure,
} from "./app-server-session";
import { CodexProcessFactory, type CodexProcessSpawnInput } from "./process";

/** Describes one Codex rate-limit window as reported by `account/rateLimits/read`. @since 0.6.0 */
export const CodexRateLimitWindow = Schema.Struct({
	resetsAt: Schema.optional(Schema.NullOr(Schema.Number)),
	usedPercent: Schema.optional(Schema.Number),
	windowDurationMins: Schema.optional(Schema.NullOr(Schema.Number)),
});

/** Represents a validated Codex rate-limit window. @since 0.6.0 */
export type CodexRateLimitWindow = Schema.Schema.Type<typeof CodexRateLimitWindow>;

/** Describes one Codex rate-limit bucket, keyed by the provider's limit identifier. @since 0.6.0 */
export const CodexRateLimitSnapshot = Schema.Struct({
	limitId: Schema.optional(Schema.NullOr(Schema.String)),
	limitName: Schema.optional(Schema.NullOr(Schema.String)),
	primary: Schema.optional(Schema.NullOr(CodexRateLimitWindow)),
	secondary: Schema.optional(Schema.NullOr(CodexRateLimitWindow)),
});

/** Represents a validated Codex rate-limit bucket. @since 0.6.0 */
export type CodexRateLimitSnapshot = Schema.Schema.Type<typeof CodexRateLimitSnapshot>;

/** Describes the permissive `account/rateLimits/read` JSON-RPC result. @since 0.6.0 */
export const CodexRateLimitsReadResult = Schema.Struct({
	rateLimits: Schema.optional(CodexRateLimitSnapshot),
	rateLimitsByLimitId: Schema.optional(
		Schema.NullOr(Schema.Record(Schema.String, CodexRateLimitSnapshot)),
	),
});

/** Represents a validated `account/rateLimits/read` JSON-RPC result. @since 0.6.0 */
export type CodexRateLimitsReadResult = Schema.Schema.Type<typeof CodexRateLimitsReadResult>;

/** Names the two rate-limit slots a Codex bucket may report. @since 0.6.0 */
const CODEX_RATE_LIMIT_SLOTS = ["primary", "secondary"] as const;

function clamp_percent_used(used_percent: number | undefined): number {
	if (used_percent === undefined || !Number.isFinite(used_percent)) {
		return 0;
	}

	return Math.min(100, Math.max(0, used_percent));
}

function classify_codex_quota_window_kind(
	window_minutes: number | undefined,
): EngineQuotaWindowKind {
	if (window_minutes === undefined) {
		return "unknown";
	}

	if (window_minutes === 300) {
		return "session";
	}

	if (window_minutes === 10_080) {
		return "weekly";
	}

	if (window_minutes >= 40_000 && window_minutes <= 45_000) {
		return "monthly";
	}

	return "unknown";
}

/**
 * Maps a decoded `account/rateLimits/read` result to provider-neutral quota
 * windows. Iterates `rateLimitsByLimitId` when present, otherwise falls back
 * to the single `rateLimits` snapshot. Emits one window per non-null
 * `primary`/`secondary` slot on each bucket. Pure and free of process I/O so
 * it can be exercised without spawning Codex.
 *
 * @since 0.6.0
 * @param result - The schema-decoded JSON-RPC result.
 * @returns Provider-neutral quota windows in bucket-then-slot order.
 */
export function map_codex_rate_limits_to_quota_windows(
	result: CodexRateLimitsReadResult,
): ReadonlyArray<EngineQuotaWindow> {
	const buckets: ReadonlyArray<readonly [string, CodexRateLimitSnapshot]> =
		result.rateLimitsByLimitId != null
			? Object.entries(result.rateLimitsByLimitId)
			: result.rateLimits !== undefined
				? [[result.rateLimits.limitId ?? "codex", result.rateLimits] as const]
				: [];
	const windows: Array<EngineQuotaWindow> = [];

	for (const [bucket_id, snapshot] of buckets) {
		const label = snapshot.limitName ?? undefined;

		for (const slot of CODEX_RATE_LIMIT_SLOTS) {
			const window = snapshot[slot];

			if (window === undefined || window === null) {
				continue;
			}

			windows.push({
				id: `${bucket_id}:${slot}`,
				kind: classify_codex_quota_window_kind(window.windowDurationMins ?? undefined),
				...(label === undefined ? {} : { label }),
				percent_used: clamp_percent_used(window.usedPercent),
				...(window.resetsAt === undefined || window.resetsAt === null
					? {}
					: { resets_at: new Date(window.resetsAt * 1_000).toISOString() }),
				...(window.windowDurationMins === undefined || window.windowDurationMins === null
					? {}
					: { window_minutes: window.windowDurationMins }),
			});
		}
	}

	return windows;
}

/** Recognizes provider error messages that report a missing or invalid login. */
const CODEX_LOGIN_ERROR_MESSAGE_PATTERN =
	/not\s+logged\s+in|log[\s-]?in\s+required|please\s+(sign|log)[\s-]?in|unauthenticated|not\s+authenticated|no\s+active\s+account|authentication\s+required/i;

function is_codex_login_error_message(message: string): boolean {
	return CODEX_LOGIN_ERROR_MESSAGE_PATTERN.test(message);
}

/** Maps app-server session failures to the engine failure channel without starting a run. */
function map_codex_usage_session_failure(
	error: CodexAppServerSessionFailure,
): EngineProcessError | EngineProtocolError {
	if (error instanceof EngineProcessError) {
		return error;
	}

	const message = (() => {
		switch (error._tag) {
			case "CodexAppServerClosedError":
				return `Codex app-server closed: ${error.reason}`;
			case "CodexAppServerConfigurationError":
				return `Invalid Codex app-server option ${error.option}: ${error.value}`;
			case "CodexAppServerProtocolError":
			case "CodexAppServerSerializationError":
				return error.message;
			case "CodexAppServerRequestTimeoutError":
				return `Codex ${error.method} request ${error.id} timed out after ${error.timeout_ms}ms`;
			case "CodexAppServerResponseError":
				return `Codex ${error.method} request ${error.id} failed (${error.error.code}): ${error.error.message}`;
		}
	})();

	return new EngineProtocolError({ engine_id: "codex", message });
}

/** Configures one non-billable Codex account-usage read. @since 0.6.0 */
export interface CodexUsageOptions {
	readonly executable: string;
	readonly executable_args: ReadonlyArray<string>;
	readonly factory: typeof CodexProcessFactory.Service;
	/** Caps the whole spawn-handshake-request sequence; defaults to 15 seconds. */
	readonly overall_timeout_ms?: number;
	readonly request_timeout_ms?: number;
}

/**
 * Builds the Codex adapter's `Engine.Usage` effect through the configured
 * `codex app-server --stdio` ACP surface. The provider's account and
 * rate-limit responses are the sole authentication authority; no private
 * credential file is read. The process is always reaped through a scope and
 * the complete request sequence is bounded by a timeout.
 *
 * @since 0.6.0
 * @param options - Configured executable and transport factory.
 * @returns A non-billable effect reporting authentication and quota windows.
 */
export function MakeCodexUsage(
	options: CodexUsageOptions,
): Effect.Effect<EngineAccountUsage, EngineFailure> {
	const request_timeout_ms = options.request_timeout_ms ?? 10_000;
	const overall_timeout_ms = options.overall_timeout_ms ?? 15_000;
	const spawn: CodexProcessSpawnInput = {
		args: [...options.executable_args, "app-server", "--stdio"],
		command: options.executable,
	};
	const RunRateLimitsRead = Effect.gen(function* () {
		const session = yield* open_codex_app_server_session({ request_timeout_ms, spawn }).pipe(
			Effect.provideService(CodexProcessFactory, options.factory),
			Effect.mapError(map_codex_usage_session_failure),
		);

		yield* session
			.Handshake({ client_name: "artisan-usage", client_version: "0.6.0" })
			.pipe(Effect.mapError(map_codex_usage_session_failure));

		const account_outcome = yield* session.Request("account/read", {}).pipe(
			Effect.map((response) => ({ _tag: "ok" as const, response })),
			Effect.catch((failure) =>
				failure instanceof CodexAppServerResponseError &&
				is_codex_login_error_message(failure.error.message)
					? Effect.succeed({
							_tag: "unauthenticated" as const,
							reason: failure.error.message,
						})
					: Effect.fail(map_codex_usage_session_failure(failure)),
			),
		);

		if (account_outcome._tag === "unauthenticated") {
			return {
				authentication: {
					reason: account_outcome.reason,
					state: "unauthenticated" as const,
				},
				windows: [],
			} satisfies EngineAccountUsage;
		}

		const account = yield* Schema.decodeUnknownEffect(CodexAccountReadSchema, {
			onExcessProperty: "preserve",
		})(account_outcome.response.result).pipe(
			Effect.mapError(
				() =>
					new EngineProtocolError({
						engine_id: "codex",
						message: "Codex account/read returned an invalid result",
					}),
			),
		);

		if (account.account === null) {
			return {
				authentication: {
					reason: account.requiresOpenaiAuth
						? "OpenAI authentication required"
						: "No ChatGPT or API-key account is active",
					state: "unauthenticated" as const,
				},
				windows: [],
			} satisfies EngineAccountUsage;
		}

		const outcome = yield* session.Request("account/rateLimits/read", {}).pipe(
			Effect.map((response) => ({ _tag: "ok" as const, response })),
			Effect.catch((failure) =>
				failure instanceof CodexAppServerResponseError &&
				is_codex_login_error_message(failure.error.message)
					? Effect.succeed({
							_tag: "unauthenticated" as const,
							reason: failure.error.message,
						})
					: Effect.fail(map_codex_usage_session_failure(failure)),
			),
		);

		if (outcome._tag === "unauthenticated") {
			return {
				authentication: { reason: outcome.reason, state: "unauthenticated" as const },
				windows: [],
			} satisfies EngineAccountUsage;
		}

		const decoded = yield* Schema.decodeUnknownEffect(CodexRateLimitsReadResult, {
			onExcessProperty: "preserve",
		})(outcome.response.result).pipe(
			Effect.mapError(
				() =>
					new EngineProtocolError({
						engine_id: "codex",
						message: "Codex account/rateLimits/read returned an invalid result",
					}),
			),
		);

		const account_email =
			account.account.type === "chatgpt" ? (account.account.email ?? undefined) : undefined;

		return {
			...(account_email === undefined ? {} : { account_email }),
			authentication: { state: "authenticated" as const },
			windows: map_codex_rate_limits_to_quota_windows(decoded),
		} satisfies EngineAccountUsage;
	});

	return Effect.scoped(RunRateLimitsRead).pipe(
		Effect.timeoutOrElse({
			duration: overall_timeout_ms,
			orElse: () =>
				Effect.fail(
					new EngineProcessError({
						cause: new Error(
							`Codex account usage timed out after ${overall_timeout_ms}ms`,
						),
						operation: "read",
					}),
				),
		}),
	);
}
