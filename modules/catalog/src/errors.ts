import { Schema } from "effect";

/**
 * A stable Artisan error code: `AE-<AREA>-<NNN>`. Codes are Artisan's own
 * error identity — engine adapters translate provider-native signals into a
 * code at the boundary, and everything downstream (persistence, protocol,
 * rendering, future docs links) speaks only this vocabulary. Areas name the
 * fault domain — where the problem lives, not what subsystem noticed it:
 * `CLIENT_STATE` is this machine (sign-in, credentials, engine startup),
 * `PROVIDER` is their side (limits, billing, overload, rejections), `RUN` is
 * the run itself. The pattern is validated but the set is open: a renderer
 * must tolerate a code minted after it shipped and fall back to the unknown
 * definition.
 */
const artisan_error_code_pattern = /^AE-[A-Z]+(?:_[A-Z]+)*-\d{3}$/;

export const ArtisanErrorCode = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		artisan_error_code_pattern.test(value)
			? undefined
			: "Expected an Artisan error code shaped AE-<AREA>-<NNN>",
	),
);
export type ArtisanErrorCode = typeof ArtisanErrorCode.Type;

/**
 * The renderable identity of one Artisan error: what the failure is called,
 * what it means for the user, and whether retrying the work is a sensible
 * offer. `docs_slug` names the future documentation page for the code; a
 * definition without one simply renders no link.
 */
export interface ArtisanErrorDefinition {
	readonly code: ArtisanErrorCode;
	readonly docs_slug?: string;
	readonly retryable: boolean;
	readonly summary: string;
	readonly title: string;
}

/**
 * Semantic names for every minted code, so call sites read as intent rather
 * than numerology. Adding an error means adding a name here and a definition
 * below; the completeness test keeps the two in lockstep.
 */
export const artisan_error_codes = {
	engine_not_signed_in: "AE-CLIENT_STATE-101",
	provider_auth_failed: "AE-CLIENT_STATE-102",
	provider_org_not_allowed: "AE-CLIENT_STATE-103",
	engine_unavailable: "AE-CLIENT_STATE-104",
	engine_start_failed: "AE-CLIENT_STATE-105",
	engine_start_timeout: "AE-CLIENT_STATE-106",
	working_directory_missing: "AE-CLIENT_STATE-107",
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
} as const satisfies Record<string, ArtisanErrorCode>;

const definitions: ReadonlyArray<ArtisanErrorDefinition> = [
	{
		code: artisan_error_codes.engine_not_signed_in,
		docs_slug: "engine-not-signed-in",
		retryable: false,
		summary:
			"The engine has no usable sign-in on this machine. Sign in with the provider's own tooling, then send the message again.",
		title: "Engine not signed in",
	},
	{
		code: artisan_error_codes.provider_auth_failed,
		docs_slug: "provider-auth-failed",
		retryable: true,
		summary:
			"The provider rejected this machine's sign-in — usually an expired credential that could not refresh. The sign-in is shared by every app using it, and the condition often clears within a minute.",
		title: "Provider sign-in failed",
	},
	{
		code: artisan_error_codes.provider_org_not_allowed,
		docs_slug: "provider-org-not-allowed",
		retryable: false,
		summary:
			"The signed-in account's organization is not allowed to use this model or feature.",
		title: "Organization not allowed",
	},
	{
		code: artisan_error_codes.engine_unavailable,
		docs_slug: "engine-unavailable",
		retryable: true,
		summary:
			"The engine could not start a session. Its readiness check failed — the reason it reported is in the details below.",
		title: "Engine unavailable",
	},
	{
		code: artisan_error_codes.engine_start_failed,
		docs_slug: "engine-start-failed",
		retryable: true,
		summary:
			"The engine failed before its native session became ready. The failure it reported is in the details below.",
		title: "Engine startup failed",
	},
	{
		code: artisan_error_codes.engine_start_timeout,
		docs_slug: "engine-start-timeout",
		retryable: true,
		summary: "The engine did not become ready within the startup deadline.",
		title: "Engine startup timed out",
	},
	{
		code: artisan_error_codes.working_directory_missing,
		docs_slug: "working-directory-missing",
		/**
		 * Retrying re-launches into the same missing folder, so the offer would
		 * only replay the failure. The remedy is restoring the folder or pointing
		 * the thread at a project that exists, and only the user can do either.
		 */
		retryable: false,
		summary:
			"The thread's working directory no longer exists on this machine. Restore the folder — or move the project back — then send the message again.",
		title: "Working directory missing",
	},
	{
		code: artisan_error_codes.usage_limit_reached,
		docs_slug: "usage-limit-reached",
		retryable: true,
		summary:
			"The provider's usage limit for this plan window has been reached. Work resumes when the window resets.",
		title: "Usage limit reached",
	},
	{
		code: artisan_error_codes.provider_overloaded,
		docs_slug: "provider-overloaded",
		retryable: true,
		summary:
			"The provider is temporarily overloaded. This is transient on their side — retrying shortly usually succeeds.",
		title: "Provider overloaded",
	},
	{
		code: artisan_error_codes.provider_billing_problem,
		docs_slug: "provider-billing-problem",
		retryable: false,
		summary:
			"The provider reported a billing problem with the signed-in account. Review the account's plan or payment settings.",
		title: "Provider billing problem",
	},
	{
		code: artisan_error_codes.request_rejected,
		docs_slug: "request-rejected",
		retryable: false,
		summary:
			"The provider rejected the request as invalid. The exact reason is in the details below.",
		title: "Request rejected",
	},
	{
		code: artisan_error_codes.model_unavailable,
		docs_slug: "model-unavailable",
		retryable: false,
		summary:
			"The selected model is not available to this account or no longer exists. Pick another model and try again.",
		title: "Model unavailable",
	},
	{
		code: artisan_error_codes.provider_server_error,
		docs_slug: "provider-server-error",
		retryable: true,
		summary:
			"The provider hit an internal error while serving the request. Nothing on this machine caused it — retrying usually succeeds.",
		title: "Provider server error",
	},
	{
		code: artisan_error_codes.run_failed,
		docs_slug: "run-failed",
		retryable: true,
		summary:
			"The run failed while executing. The engine's own account is in the details below.",
		title: "Run failed",
	},
	{
		code: artisan_error_codes.run_turn_limit,
		docs_slug: "run-turn-limit",
		retryable: false,
		summary:
			"The run stopped after reaching its configured turn limit before finishing the task.",
		title: "Turn limit reached",
	},
	{
		code: artisan_error_codes.run_budget_exhausted,
		docs_slug: "run-budget-exhausted",
		retryable: false,
		summary: "The run stopped after exhausting its configured spending budget.",
		title: "Budget exhausted",
	},
	{
		code: artisan_error_codes.run_structured_output_failed,
		docs_slug: "run-structured-output-failed",
		retryable: true,
		summary:
			"The engine could not produce output matching the requested structure within its retry allowance.",
		title: "Structured output failed",
	},
	{
		code: artisan_error_codes.run_output_limit,
		docs_slug: "run-output-limit",
		retryable: true,
		summary: "The reply was cut off by the provider's output-length limit.",
		title: "Output limit reached",
	},
	{
		code: artisan_error_codes.unknown,
		retryable: false,
		summary:
			"The engine reported a failure Artisan does not recognize yet. The raw report is in the details below.",
		title: "Unexpected engine failure",
	},
];

const definitions_by_code = new Map(definitions.map((definition) => [definition.code, definition]));

/**
 * Every code renders through this lookup, so an unrecognized code — minted by
 * a newer backend than the renderer — degrades to the unknown definition
 * instead of an empty card. The raw code still displays, because it is the
 * durable artifact a future docs page will be keyed on.
 */
export const lookup_artisan_error = (code: string): ArtisanErrorDefinition => {
	const known = definitions_by_code.get(code);
	if (known !== undefined) return known;
	const unknown = definitions_by_code.get(artisan_error_codes.unknown) as ArtisanErrorDefinition;
	return artisan_error_code_pattern.test(code) ? { ...unknown, code } : unknown;
};

/** The full registry, exposed for docs generation and completeness tests. */
export const artisan_error_definitions = definitions;
