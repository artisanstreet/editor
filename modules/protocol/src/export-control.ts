import { Schema } from "effect";

import { Identifier, IsoDateTime, PositiveInt } from "./common";

/** Enumerates Artisan-operated actions that require an export-control decision. */
export const ExportControlAction = Schema.Literals([
	"account",
	"billing",
	"distribution",
	"hosted_sync",
	"marketplace_delivery",
	"release",
	"update",
]);

export type ExportControlAction = typeof ExportControlAction.Type;

/** Enumerates reliable jurisdiction signals without accepting device locale. */
export const ExportControlSignalKind = Schema.Literals([
	"account_country",
	"billing_country",
	"network_country",
]);

export type ExportControlSignalKind = typeof ExportControlSignalKind.Type;

/** Represents an ISO 3166-1 alpha-2 country code used only during evaluation. */
export const ExportControlCountryCode = Schema.String.check(
	Schema.isPattern(/^[A-Z]{2}$/u, { message: "Expected an uppercase country code" }),
);

export type ExportControlCountryCode = typeof ExportControlCountryCode.Type;

/** Carries one minimum jurisdiction signal into the compliance boundary. */
export const ExportControlSignal = Schema.Struct({
	country_code: ExportControlCountryCode,
	kind: ExportControlSignalKind,
});

export type ExportControlSignal = typeof ExportControlSignal.Type;

/** Requests one exact-replay compliance decision. */
export const ExportControlCheckRequest = Schema.Struct({
	action: ExportControlAction,
	decision_id: Identifier,
	signals: Schema.Array(ExportControlSignal).check(Schema.isMaxLength(3)),
});

export type ExportControlCheckRequest = typeof ExportControlCheckRequest.Type;

/** Declares the minimum reliable signals required for one protected action. */
export const ExportControlActionRequirement = Schema.Struct({
	action: ExportControlAction,
	required_signal_kinds: Schema.NonEmptyArray(ExportControlSignalKind).check(
		Schema.isMaxLength(3),
	),
});

export type ExportControlActionRequirement = typeof ExportControlActionRequirement.Type;

/** Describes the legally reviewed metadata attached to a policy revision. */
export const ExportControlLegalReview = Schema.Struct({
	approved_at: IsoDateTime,
	expires_at: IsoDateTime,
	reference: Identifier,
	status: Schema.Literal("approved"),
});

export type ExportControlLegalReview = typeof ExportControlLegalReview.Type;

/** Defines one externally supplied, versioned export-control policy revision. */
export const ExportControlPolicy = Schema.Struct({
	action_requirements: Schema.NonEmptyArray(ExportControlActionRequirement).check(
		Schema.isMaxLength(7),
	),
	denied_country_codes: Schema.NonEmptyArray(ExportControlCountryCode).check(
		Schema.isMaxLength(256),
	),
	effective_at: IsoDateTime,
	expires_at: IsoDateTime,
	legal_review: ExportControlLegalReview,
	policy_id: Identifier,
	schema_version: Schema.Literal(1),
	support_url: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(2_048)),
	version: PositiveInt,
});

export type ExportControlPolicy = typeof ExportControlPolicy.Type;

const ExportControlDecisionMetadata = {
	decision_id: Identifier,
	policy_id: Identifier,
	policy_version: PositiveInt,
};

/** Allows one protected action under an exact current policy revision. */
export const ExportControlAllowedDecision = Schema.Struct({
	...ExportControlDecisionMetadata,
	decision: Schema.Literal("allowed"),
});

/** Denies one protected action without exposing the matching screening signal. */
export const ExportControlRestrictedDecision = Schema.Struct({
	...ExportControlDecisionMetadata,
	code: Schema.Literal("restricted_region"),
	decision: Schema.Literal("restricted"),
	support_url: Schema.String.check(Schema.isMinLength(1), Schema.isMaxLength(2_048)),
});

/** Fails closed when no complete, current, legally reviewed decision can be made. */
export const ExportControlUnavailableDecision = Schema.Struct({
	code: Schema.Literals([
		"audit_unavailable",
		"insufficient_signals",
		"invalid_policy",
		"policy_unavailable",
	]),
	decision: Schema.Literal("unavailable"),
	decision_id: Identifier,
	policy_id: Schema.optional(Identifier),
	policy_version: Schema.optional(PositiveInt),
});

/** Represents every source-safe export-control decision. */
export const ExportControlDecision = Schema.Union([
	ExportControlAllowedDecision,
	ExportControlRestrictedDecision,
	ExportControlUnavailableDecision,
]);

export type ExportControlDecision = typeof ExportControlDecision.Type;

/** Records only privacy-bounded evidence that a compliance decision occurred. */
export const ExportControlAuditRecord = Schema.Struct({
	action: ExportControlAction,
	decision: Schema.Literals(["allowed", "restricted", "unavailable"]),
	decision_id: Identifier,
	occurred_at: IsoDateTime,
	policy_id: Schema.optional(Identifier),
	policy_version: Schema.optional(PositiveInt),
	reason_code: Schema.Literals([
		"allowed",
		"audit_unavailable",
		"insufficient_signals",
		"invalid_policy",
		"policy_unavailable",
		"restricted_region",
	]),
	signal_kinds: Schema.Array(ExportControlSignalKind).check(Schema.isMaxLength(3)),
});

export type ExportControlAuditRecord = typeof ExportControlAuditRecord.Type;
