import { artisan_error_codes } from "@artisan/catalog";

import type { EngineErrorRef } from "../engine";

/**
 * The Agent SDK's typed per-message error enum (`SDKAssistantMessageError`),
 * translated into Artisan's error vocabulary. The provider value rides along
 * as `provider_code`, so the original signal is never lost — only custody
 * changes. An enum value added by a newer SDK maps to the unknown code and
 * still surfaces as a classified failure rather than a bare string.
 */
const assistant_error_codes: Readonly<Record<string, string>> = {
	authentication_failed: artisan_error_codes.provider_auth_failed,
	billing_error: artisan_error_codes.provider_billing_problem,
	invalid_request: artisan_error_codes.request_rejected,
	max_output_tokens: artisan_error_codes.run_output_limit,
	model_not_found: artisan_error_codes.model_unavailable,
	oauth_org_not_allowed: artisan_error_codes.provider_org_not_allowed,
	overloaded: artisan_error_codes.provider_overloaded,
	rate_limit: artisan_error_codes.usage_limit_reached,
	server_error: artisan_error_codes.provider_server_error,
	unknown: artisan_error_codes.unknown,
};

/** The CLI's terminal result failure subtypes (`SDKResultError["subtype"]`). */
const result_failure_codes: Readonly<Record<string, string>> = {
	error_during_execution: artisan_error_codes.run_failed,
	error_max_budget_usd: artisan_error_codes.run_budget_exhausted,
	error_max_structured_output_retries: artisan_error_codes.run_structured_output_failed,
	error_max_turns: artisan_error_codes.run_turn_limit,
};

/** Classifies a typed assistant-message error into Artisan custody. @since 0.9.0 */
export const classify_claude_assistant_error = (provider_code: string): EngineErrorRef => ({
	artisan_code: assistant_error_codes[provider_code] ?? artisan_error_codes.unknown,
	provider_code,
});

/** Classifies a failed result frame's subtype into Artisan custody. @since 0.9.0 */
export const classify_claude_result_failure = (subtype: string): EngineErrorRef => ({
	artisan_code: result_failure_codes[subtype] ?? artisan_error_codes.unknown,
	provider_code: subtype,
});

/** Classifies a terminal failure that disclosed only a stop reason. @since 0.9.0 */
export const classify_claude_terminal_failure = (
	stop_reason: string | undefined,
): EngineErrorRef => ({
	artisan_code: artisan_error_codes.run_failed,
	...(stop_reason === undefined ? {} : { provider_code: stop_reason }),
});

/**
 * Classifies a rejected rate-limit event, carrying the reset instant when the
 * SDK disclosed one. The SDK reports epoch seconds; anything already in
 * milliseconds is recognized by magnitude rather than trusted blindly.
 */
export const classify_claude_rate_limit = (resets_at?: number): EngineErrorRef => ({
	artisan_code: artisan_error_codes.usage_limit_reached,
	provider_code: "rate_limit",
	...(resets_at === undefined || !Number.isFinite(resets_at)
		? {}
		: {
				resets_at: new Date(
					resets_at > 1_000_000_000_000 ? resets_at : resets_at * 1_000,
				).toISOString(),
			}),
});
