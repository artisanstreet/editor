import type { EngineErrorRef } from "../engine";

/**
 * Stable public error identifiers emitted by the Claude adapter. These are
 * wire-level values, not a dependency on the renderer's catalog: engines
 * classifies native failures before any catalog or presentation layer exists.
 * The catalog owns the human-readable definitions for the same stable values.
 */
export const claude_artisan_error_codes = {
	engine_not_signed_in: "AE-CLIENT_STATE-101",
	provider_auth_failed: "AE-CLIENT_STATE-102",
	provider_org_not_allowed: "AE-CLIENT_STATE-103",
	usage_limit_reached: "AE-PROVIDER-201",
	provider_overloaded: "AE-PROVIDER-202",
	provider_billing_problem: "AE-PROVIDER-203",
	provider_server_error: "AE-PROVIDER-204",
	request_rejected: "AE-PROVIDER-205",
	model_unavailable: "AE-PROVIDER-206",
	run_failed: "AE-RUN-301",
	run_turn_limit: "AE-RUN-302",
	run_budget_exhausted: "AE-RUN-303",
	run_structured_output_failed: "AE-RUN-304",
	run_output_limit: "AE-RUN-305",
	unknown: "AE-UNKNOWN-000",
} as const;

/**
 * Claude's assistant-message error codes translated into Artisan's error
 * vocabulary. The provider value rides along as `provider_code`, so a new CLI
 * value maps to the unknown code without losing the original signal.
 */
const assistant_error_codes: Readonly<Record<string, string>> = {
	authentication_failed: claude_artisan_error_codes.provider_auth_failed,
	billing_error: claude_artisan_error_codes.provider_billing_problem,
	invalid_request: claude_artisan_error_codes.request_rejected,
	max_output_tokens: claude_artisan_error_codes.run_output_limit,
	model_not_found: claude_artisan_error_codes.model_unavailable,
	oauth_org_not_allowed: claude_artisan_error_codes.provider_org_not_allowed,
	overloaded: claude_artisan_error_codes.provider_overloaded,
	rate_limit: claude_artisan_error_codes.usage_limit_reached,
	server_error: claude_artisan_error_codes.provider_server_error,
	unknown: claude_artisan_error_codes.unknown,
};

/** The CLI's terminal result failure subtypes. */
const result_failure_codes: Readonly<Record<string, string>> = {
	error_during_execution: claude_artisan_error_codes.run_failed,
	error_max_budget_usd: claude_artisan_error_codes.run_budget_exhausted,
	error_max_structured_output_retries: claude_artisan_error_codes.run_structured_output_failed,
	error_max_turns: claude_artisan_error_codes.run_turn_limit,
};

/** Classifies a typed assistant-message error into Artisan custody. @since 0.9.0 */
export const classify_claude_assistant_error = (provider_code: string): EngineErrorRef => ({
	artisan_code: assistant_error_codes[provider_code] ?? claude_artisan_error_codes.unknown,
	provider_code,
});

/** Classifies a failed result frame's subtype into Artisan custody. @since 0.9.0 */
export const classify_claude_result_failure = (subtype: string): EngineErrorRef => ({
	artisan_code: result_failure_codes[subtype] ?? claude_artisan_error_codes.unknown,
	provider_code: subtype,
});

/** Classifies a terminal failure that disclosed only a stop reason. @since 0.9.0 */
export const classify_claude_terminal_failure = (
	stop_reason: string | undefined,
): EngineErrorRef => ({
	artisan_code: claude_artisan_error_codes.run_failed,
	...(stop_reason === undefined ? {} : { provider_code: stop_reason }),
});

/**
 * Classifies a rejected rate-limit event, carrying the reset instant when the
 * provider stream disclosed one. Claude reports epoch seconds; anything already in
 * milliseconds is recognized by magnitude rather than trusted blindly.
 */
export const classify_claude_rate_limit = (resets_at?: number): EngineErrorRef => ({
	artisan_code: claude_artisan_error_codes.usage_limit_reached,
	provider_code: "rate_limit",
	...(resets_at === undefined || !Number.isFinite(resets_at)
		? {}
		: {
				resets_at: new Date(
					resets_at > 1_000_000_000_000 ? resets_at : resets_at * 1_000,
				).toISOString(),
			}),
});
