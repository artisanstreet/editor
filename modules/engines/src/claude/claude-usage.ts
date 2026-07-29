import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";

import { Cause, Effect, Option, Schema } from "effect";

import {
	type EngineAccountUsage,
	type EngineFailure,
	type EngineQuotaWindow,
	type EngineQuotaWindowKind,
	EngineProtocolError,
	EngineUnavailableError,
} from "../engine";

/** Configures how {@link MakeClaudeUsage} locates the CLI's saved OAuth session. @since 0.6.0 */
export interface ClaudeUsageOptions {
	/**
	 * Overrides Claude's config-dir resolution (normally env `CLAUDE_CONFIG_DIR`,
	 * else `join(homedir(), ".claude")`). Exists primarily so tests can inject a
	 * temporary directory instead of touching the real credentials file.
	 */
	readonly claude_config_dir?: string;
}

/** Mirrors the on-disk shape of `<claude_config_dir>/.credentials.json`; unknown keys are ignored. */
const ClaudeOauthCredentials = Schema.Struct({
	accessToken: Schema.optional(Schema.String),
	expiresAt: Schema.optional(Schema.Number),
	rateLimitTier: Schema.optional(Schema.String),
	refreshToken: Schema.optional(Schema.String),
	scopes: Schema.optional(Schema.Array(Schema.String)),
	subscriptionType: Schema.optional(Schema.String),
});
const ClaudeCredentialsFile = Schema.Struct({
	claudeAiOauth: Schema.optional(ClaudeOauthCredentials),
});

/** Names the model a `weekly_scoped` limit applies to, when the provider discloses one. */
const ClaudeUsageLimitScope = Schema.Struct({
	model: Schema.optional(
		Schema.Struct({
			display_name: Schema.optional(Schema.String),
			id: Schema.optional(Schema.String),
		}),
	),
});

/** One canonical entry from the provider's `limits[]` array. */
const ClaudeUsageLimit = Schema.Struct({
	is_active: Schema.optional(Schema.Boolean),
	kind: Schema.optional(Schema.String),
	percent: Schema.optional(Schema.Number),
	resets_at: Schema.optional(Schema.String),
	scope: Schema.optional(Schema.NullOr(ClaudeUsageLimitScope)),
	severity: Schema.optional(Schema.String),
});
type ClaudeUsageLimit = typeof ClaudeUsageLimit.Type;

/** A coarse `five_hour` or `seven_day` usage summary; superseded by `limits[]` when present. */
const ClaudeUsageWindowSummary = Schema.Struct({
	resets_at: Schema.optional(Schema.NullOr(Schema.String)),
	utilization: Schema.optional(Schema.NullOr(Schema.Number)),
});

/**
 * Decodes the body of `GET https://api.anthropic.com/api/oauth/usage`.
 * Every field is optional-tolerant because the provider omits windows that do
 * not apply to the caller's plan; unknown keys are ignored rather than
 * rejected.
 */
export const ClaudeUsageResponseSchema = Schema.Struct({
	five_hour: Schema.optional(Schema.NullOr(ClaudeUsageWindowSummary)),
	limits: Schema.optional(Schema.Array(ClaudeUsageLimit)),
	seven_day: Schema.optional(Schema.NullOr(ClaudeUsageWindowSummary)),
});
export type ClaudeUsageResponse = typeof ClaudeUsageResponseSchema.Type;

const claude_usage_endpoint = "https://api.anthropic.com/api/oauth/usage";
/** Matches the Claude CLI's own User-Agent so the endpoint sees a recognized client. */
const claude_usage_user_agent = "claude-cli/2.1.220 (external, cli)";
const claude_usage_oauth_beta = "oauth-2025-04-20";
const claude_usage_timeout = "10 seconds";

function clamp_percent(value: number): number {
	return Math.min(100, Math.max(0, value));
}

/** Drops a timestamp that does not parse as a real date, re-serialized to the repo's IsoDateTime shape. */
function normalize_resets_at(value: string | null | undefined): string | undefined {
	if (value === null || value === undefined) return undefined;
	const parsed = new Date(value);
	return Number.isNaN(parsed.getTime()) ? undefined : parsed.toISOString();
}

function limit_window_kind(kind: string): EngineQuotaWindowKind {
	if (kind === "session") return "session";
	if (kind === "weekly_all" || kind === "weekly_scoped") return "weekly";
	return "unknown";
}

/** Maps one `limits[]` entry, or `undefined` when it lacks the fields needed to report it. */
function map_claude_limit(limit: ClaudeUsageLimit): EngineQuotaWindow | undefined {
	if (limit.kind === undefined || limit.percent === undefined) return undefined;
	const model_id = limit.scope?.model?.id;
	const label = limit.scope?.model?.display_name;
	const resets_at = normalize_resets_at(limit.resets_at);
	return {
		id: model_id === undefined ? limit.kind : `${limit.kind}:${model_id}`,
		kind: limit_window_kind(limit.kind),
		percent_used: clamp_percent(limit.percent),
		...(label === undefined ? {} : { label }),
		...(resets_at === undefined ? {} : { resets_at }),
	};
}

/**
 * Maps a decoded usage response to canonical quota windows. Prefers the
 * per-model `limits[]` array when it is present and non-empty; otherwise
 * falls back to the coarse `five_hour`/`seven_day` summary fields.
 */
function map_claude_quota_windows(response: ClaudeUsageResponse): ReadonlyArray<EngineQuotaWindow> {
	const limits = response.limits ?? [];
	if (limits.length > 0) return limits.flatMap((limit) => map_claude_limit(limit) ?? []);

	const windows: Array<EngineQuotaWindow> = [];
	const five_hour = response.five_hour;
	if (five_hour?.utilization !== undefined && five_hour.utilization !== null) {
		const resets_at = normalize_resets_at(five_hour.resets_at);
		windows.push({
			id: "five_hour",
			kind: "session",
			percent_used: clamp_percent(five_hour.utilization),
			window_minutes: 300,
			...(resets_at === undefined ? {} : { resets_at }),
		});
	}
	const seven_day = response.seven_day;
	if (seven_day?.utilization !== undefined && seven_day.utilization !== null) {
		const resets_at = normalize_resets_at(seven_day.resets_at);
		windows.push({
			id: "seven_day",
			kind: "weekly",
			percent_used: clamp_percent(seven_day.utilization),
			window_minutes: 10080,
			...(resets_at === undefined ? {} : { resets_at }),
		});
	}
	return windows;
}

/**
 * Pure mapping from a schema-decoded usage response to the provider-neutral
 * account usage shape. Holds no I/O; a successful decode always implies an
 * authenticated caller because the provider only returns this body for a
 * valid session.
 */
export function map_claude_account_usage(response: ClaudeUsageResponse): EngineAccountUsage {
	return {
		authentication: { state: "authenticated" },
		windows: map_claude_quota_windows(response),
	};
}

function resolve_claude_credentials_path(options: ClaudeUsageOptions): string {
	const config_dir =
		options.claude_config_dir ?? process.env.CLAUDE_CONFIG_DIR ?? join(homedir(), ".claude");
	return join(config_dir, ".credentials.json");
}

/**
 * Reads the CLI's saved OAuth access token. A missing file, unreadable file,
 * malformed JSON, or absent `accessToken` all resolve to `undefined` rather
 * than a failure, since an unauthenticated account is a value this adapter
 * must report, not an error.
 */
function read_claude_access_token(credentials_path: string): Effect.Effect<string | undefined> {
	return Effect.tryPromise({
		try: () => readFile(credentials_path, "utf8"),
		catch: () => "unreadable" as const,
	}).pipe(
		Effect.map((text) => {
			try {
				const parsed = JSON.parse(text) as unknown;
				return Option.getOrUndefined(
					Schema.decodeUnknownOption(ClaudeCredentialsFile)(parsed),
				)?.claudeAiOauth?.accessToken;
			} catch {
				return undefined;
			}
		}),
		Effect.catch(() => Effect.succeed(undefined)),
	);
}

/** Distinguishes an authenticated fetch from the 401 case, which is a value rather than a failure. */
type ClaudeUsageFetchOutcome =
	| { readonly _tag: "ok"; readonly body: unknown }
	| { readonly _tag: "unauthenticated"; readonly reason: string };

/**
 * Calls the OAuth usage endpoint with the caller's access token. Never
 * attempts a token refresh on 401: racing the CLI's own refresh-token
 * rotation would de-authenticate the user, so a 401 is reported as
 * unauthenticated instead. HTTP 429 and other non-2xx statuses, network
 * failures, and invalid JSON all become a typed `EngineFailure`.
 */
function fetch_claude_usage(
	access_token: string,
): Effect.Effect<ClaudeUsageFetchOutcome, EngineFailure> {
	return Effect.tryPromise({
		try: () =>
			globalThis.fetch(claude_usage_endpoint, {
				headers: {
					Authorization: `Bearer ${access_token}`,
					"User-Agent": claude_usage_user_agent,
					"anthropic-beta": claude_usage_oauth_beta,
				},
			}),
		catch: (cause) =>
			new EngineUnavailableError({
				engine_id: "claude",
				message: `Claude usage request failed: ${String(cause)}`,
			}),
	}).pipe(
		Effect.flatMap((response): Effect.Effect<ClaudeUsageFetchOutcome, EngineFailure> => {
			if (response.status === 401)
				return Effect.succeed({
					_tag: "unauthenticated" as const,
					reason: "token expired or revoked",
				});
			if (!response.ok)
				return Effect.fail(
					new EngineUnavailableError({
						engine_id: "claude",
						message: `Claude usage endpoint responded with status ${response.status}`,
					}),
				);
			return Effect.tryPromise({
				try: () => response.json() as Promise<unknown>,
				catch: (cause) =>
					new EngineProtocolError({
						engine_id: "claude",
						message: `Claude usage response was not valid JSON: ${String(cause)}`,
					}),
			}).pipe(Effect.map((body) => ({ _tag: "ok" as const, body })));
		}),
		Effect.timeout(claude_usage_timeout),
		Effect.mapError((cause: EngineFailure | Cause.TimeoutError) =>
			cause._tag === "EngineProtocolError" || cause._tag === "EngineUnavailableError"
				? cause
				: new EngineUnavailableError({
						engine_id: "claude",
						message: `Claude usage request timed out after ${claude_usage_timeout}`,
					}),
		),
	);
}

/**
 * Reports Claude's provider-account quota windows without starting a run.
 * Resolves credentials from `<claude_config_dir>/.credentials.json`, never
 * logging or embedding the token, and never attempts a refresh.
 *
 * @since 0.6.0
 */
export function MakeClaudeUsage(
	options: ClaudeUsageOptions = {},
): Effect.Effect<EngineAccountUsage, EngineFailure> {
	return Effect.gen(function* () {
		const credentials_path = resolve_claude_credentials_path(options);
		const access_token = yield* read_claude_access_token(credentials_path);

		if (access_token === undefined)
			return {
				authentication: {
					reason: "No Claude Code subscription session was found",
					state: "unauthenticated",
				},
				windows: [],
			} satisfies EngineAccountUsage;

		const outcome = yield* fetch_claude_usage(access_token);

		if (outcome._tag === "unauthenticated")
			return {
				authentication: { reason: outcome.reason, state: "unauthenticated" },
				windows: [],
			} satisfies EngineAccountUsage;

		const decoded = Schema.decodeUnknownOption(ClaudeUsageResponseSchema)(outcome.body);
		if (Option.isNone(decoded))
			return yield* Effect.fail(
				new EngineProtocolError({
					engine_id: "claude",
					message: "Claude usage response did not match the expected shape",
				}),
			);

		return map_claude_account_usage(decoded.value);
	});
}
