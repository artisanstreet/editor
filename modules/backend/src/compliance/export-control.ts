import { Context, Data, DateTime, Effect, Layer, Option, Schema } from "effect";

import {
	ExportControlAuditRecord,
	ExportControlCheckRequest,
	ExportControlDecision,
	ExportControlPolicy,
	type ExportControlAuditRecord as ExportControlAuditRecordValue,
	type ExportControlCheckRequest as ExportControlCheckRequestValue,
	type ExportControlDecision as ExportControlDecisionValue,
	type ExportControlPolicy as ExportControlPolicyValue,
} from "@artisan/protocol";

export interface ExportControlAuditCommit {
	readonly decision: ExportControlDecisionValue;
	readonly intent_fingerprint: string;
	readonly record: ExportControlAuditRecordValue;
}

export class ExportControlPolicySourceFailure extends Data.TaggedError(
	"ExportControlPolicySourceFailure",
)<{
	readonly cause?: unknown;
}> {}

export class ExportControlAuditConflict extends Data.TaggedError("ExportControlAuditConflict")<{
	readonly decision_id: string;
}> {}

export class ExportControlAuditFailure extends Data.TaggedError("ExportControlAuditFailure")<{
	readonly cause?: unknown;
}> {}

export class ExportControlIntentCommitmentFailure extends Data.TaggedError(
	"ExportControlIntentCommitmentFailure",
)<{
	readonly cause?: unknown;
}> {}

export class ExportControlInputInvalid extends Data.TaggedError("ExportControlInputInvalid")<{
	readonly message: string;
}> {}

export class ExportControlRestricted extends Data.TaggedError("ExportControlRestricted")<{
	readonly decision: Extract<ExportControlDecisionValue, { decision: "restricted" }>;
}> {}

export class ExportControlUnavailable extends Data.TaggedError("ExportControlUnavailable")<{
	readonly decision: Extract<ExportControlDecisionValue, { decision: "unavailable" }>;
}> {}

export type ExportControlGateError =
	| ExportControlInputInvalid
	| ExportControlRestricted
	| ExportControlUnavailable;

/** Supplies the current externally managed policy without fixing its transport in domain code. */
export class ExportControlPolicySource extends Context.Service<
	ExportControlPolicySource,
	{
		readonly Load: Effect.Effect<unknown, ExportControlPolicySourceFailure>;
	}
>()("Artisan/ExportControlPolicySource") {}

/** Produces a non-enumerable stable commitment for privacy-sensitive decision intent. */
export class ExportControlIntentCommitment extends Context.Service<
	ExportControlIntentCommitment,
	{
		readonly Fingerprint: (
			canonical_intent: string,
		) => Effect.Effect<string, ExportControlIntentCommitmentFailure>;
	}
>()("Artisan/ExportControlIntentCommitment") {}

/** Commits privacy-bounded decisions with exact replay and changed-intent rejection. */
export class ExportControlAuditStore extends Context.Service<
	ExportControlAuditStore,
	{
		readonly Commit: (
			input: ExportControlAuditCommit,
		) => Effect.Effect<
			ExportControlDecisionValue,
			ExportControlAuditConflict | ExportControlAuditFailure
		>;
	}
>()("Artisan/ExportControlAuditStore") {}

/** Makes fail-closed decisions for Artisan-operated and distribution actions. */
export class ExportControlGate extends Context.Service<
	ExportControlGate,
	{
		readonly Check: (
			input: unknown,
		) => Effect.Effect<ExportControlDecisionValue, ExportControlInputInvalid>;
		readonly Require: (
			input: unknown,
		) => Effect.Effect<
			Extract<ExportControlDecisionValue, { decision: "allowed" }>,
			ExportControlGateError
		>;
	}
>()("Artisan/ExportControlGate") {}

const DecodeRequest = Schema.decodeUnknownEffect(ExportControlCheckRequest, {
	onExcessProperty: "error",
});

const DecodePolicy = Schema.decodeUnknownEffect(ExportControlPolicy, {
	onExcessProperty: "error",
});

const DecodeDecision = Schema.decodeUnknownEffect(ExportControlDecision, {
	onExcessProperty: "error",
});

const DecodeAuditRecord = Schema.decodeUnknownEffect(ExportControlAuditRecord, {
	onExcessProperty: "error",
});

function unique_values(values: ReadonlyArray<string>) {
	return new Set(values).size === values.length;
}

function policy_is_current(policy: ExportControlPolicyValue, now: DateTime.Utc) {
	const effective_at = DateTime.make(policy.effective_at);
	const expires_at = DateTime.make(policy.expires_at);
	const review_approved_at = DateTime.make(policy.legal_review.approved_at);
	const review_expires_at = DateTime.make(policy.legal_review.expires_at);
	const now_millis = DateTime.toEpochMillis(now);

	return Option.match(effective_at, {
		onNone: () => false,
		onSome: (effective) =>
			Option.match(expires_at, {
				onNone: () => false,
				onSome: (expiry) =>
					Option.match(review_approved_at, {
						onNone: () => false,
						onSome: (approved) =>
							Option.match(review_expires_at, {
								onNone: () => false,
								onSome: (review_expiry) =>
									DateTime.toEpochMillis(effective) <= now_millis &&
									DateTime.toEpochMillis(approved) <= now_millis &&
									now_millis < DateTime.toEpochMillis(expiry) &&
									now_millis < DateTime.toEpochMillis(review_expiry),
							}),
					}),
			}),
	});
}

function support_url_is_safe(value: string) {
	try {
		const url = new URL(value);

		return url.protocol === "https:" && !url.username && !url.password;
	} catch {
		return false;
	}
}

function policy_shape_is_valid(policy: ExportControlPolicyValue) {
	const action_names = policy.action_requirements.map(({ action }) => action);
	const requirements_are_unique = policy.action_requirements.every(({ required_signal_kinds }) =>
		unique_values(required_signal_kinds),
	);

	return (
		unique_values(action_names) &&
		requirements_are_unique &&
		unique_values(policy.denied_country_codes) &&
		support_url_is_safe(policy.support_url)
	);
}

function sorted_signal_kinds(request: ExportControlCheckRequestValue) {
	return request.signals.map(({ kind }) => kind).sort();
}

function make_unavailable(
	request: ExportControlCheckRequestValue,
	code: Extract<ExportControlDecisionValue, { decision: "unavailable" }>["code"],
	policy?: ExportControlPolicyValue,
): ExportControlDecisionValue {
	return {
		code,
		decision: "unavailable",
		decision_id: request.decision_id,
		...(policy === undefined
			? {}
			: { policy_id: policy.policy_id, policy_version: policy.version }),
	};
}

function evaluate_policy(
	request: ExportControlCheckRequestValue,
	policy: ExportControlPolicyValue,
	now: DateTime.Utc,
): ExportControlDecisionValue {
	if (!policy_shape_is_valid(policy) || !policy_is_current(policy, now)) {
		return make_unavailable(request, "invalid_policy", policy);
	}

	const requirement = policy.action_requirements.find(({ action }) => action === request.action);
	const supplied_kinds = new Set(request.signals.map(({ kind }) => kind));

	if (
		!requirement ||
		requirement.required_signal_kinds.some((kind) => !supplied_kinds.has(kind))
	) {
		return make_unavailable(request, "insufficient_signals", policy);
	}

	const denied_codes = new Set(policy.denied_country_codes);
	const restricted = request.signals.some(({ country_code }) => denied_codes.has(country_code));

	if (restricted) {
		return {
			code: "restricted_region",
			decision: "restricted",
			decision_id: request.decision_id,
			policy_id: policy.policy_id,
			policy_version: policy.version,
			support_url: policy.support_url,
		};
	}

	return {
		decision: "allowed",
		decision_id: request.decision_id,
		policy_id: policy.policy_id,
		policy_version: policy.version,
	};
}

function audit_reason(decision: ExportControlDecisionValue) {
	if (decision.decision === "allowed") {
		return "allowed" as const;
	}

	return decision.code;
}

function canonical_intent(request: ExportControlCheckRequestValue) {
	return JSON.stringify([
		request.action,
		[...request.signals]
			.sort(
				(left, right) =>
					left.kind.localeCompare(right.kind) ||
					left.country_code.localeCompare(right.country_code),
			)
			.map(({ country_code, kind }) => [kind, country_code]),
	]);
}

/** Builds a policy source whose loader can refresh independently from the client release. */
export function make_export_control_policy_source_layer(
	Load: Effect.Effect<unknown, ExportControlPolicySourceFailure>,
) {
	return Layer.succeed(ExportControlPolicySource, { Load });
}

/** Denies every protected action until a current external policy is configured. */
export const FailClosedExportControlPolicySourceLive = Layer.succeed(ExportControlPolicySource, {
	Load: Effect.fail(new ExportControlPolicySourceFailure({})),
});

/** Prevents screening until a stable OS-protected commitment key is supplied. */
export const UnavailableExportControlIntentCommitmentLive = Layer.succeed(
	ExportControlIntentCommitment,
	{
		Fingerprint: () => Effect.fail(new ExportControlIntentCommitmentFailure({})),
	},
);

/** Evaluates current policy and commits only privacy-bounded decision evidence. */
export const ExportControlGateLive = Layer.effect(
	ExportControlGate,
	Effect.gen(function* () {
		const audit_store = yield* ExportControlAuditStore;
		const intent_commitment = yield* ExportControlIntentCommitment;
		const policy_source = yield* ExportControlPolicySource;

		const Check = (input: unknown) =>
			Effect.gen(function* () {
				const request = yield* DecodeRequest(input).pipe(
					Effect.mapError(
						() =>
							new ExportControlInputInvalid({
								message: "Invalid export-control request",
							}),
					),
				);
				const signal_kinds = sorted_signal_kinds(request);

				if (!unique_values(signal_kinds)) {
					return yield* new ExportControlInputInvalid({
						message: "Export-control signal kinds must be unique",
					});
				}

				const intent_fingerprint = yield* intent_commitment
					.Fingerprint(canonical_intent(request))
					.pipe(Effect.option);

				if (Option.isNone(intent_fingerprint)) {
					return make_unavailable(request, "audit_unavailable");
				}

				const now = yield* DateTime.now;
				const policy_source_result = yield* Effect.option(policy_source.Load);
				const policy_result = Option.isSome(policy_source_result)
					? yield* Effect.option(DecodePolicy(policy_source_result.value))
					: Option.none<ExportControlPolicyValue>();
				const policy = Option.isSome(policy_result) ? policy_result.value : undefined;
				const decision = yield* DecodeDecision(
					Option.isNone(policy_source_result)
						? make_unavailable(request, "policy_unavailable")
						: policy === undefined
							? make_unavailable(request, "invalid_policy")
							: evaluate_policy(request, policy, now),
				).pipe(
					Effect.mapError(
						() =>
							new ExportControlInputInvalid({
								message: "Invalid export-control decision",
							}),
					),
				);
				const record = yield* DecodeAuditRecord({
					action: request.action,
					decision: decision.decision,
					decision_id: request.decision_id,
					occurred_at: DateTime.formatIso(now),
					...(decision.policy_id === undefined ? {} : { policy_id: decision.policy_id }),
					...(decision.policy_version === undefined
						? {}
						: { policy_version: decision.policy_version }),
					reason_code: audit_reason(decision),
					signal_kinds,
				}).pipe(
					Effect.mapError(
						() =>
							new ExportControlInputInvalid({
								message: "Invalid export-control audit",
							}),
					),
				);
				const committed = yield* audit_store
					.Commit({ decision, intent_fingerprint: intent_fingerprint.value, record })
					.pipe(Effect.option);

				if (Option.isNone(committed)) {
					return make_unavailable(request, "audit_unavailable", policy);
				}

				return committed.value;
			});

		const Require = (input: unknown) =>
			Effect.gen(function* () {
				const decision = yield* Check(input);

				if (decision.decision === "allowed") {
					return decision;
				}

				if (decision.decision === "restricted") {
					return yield* new ExportControlRestricted({ decision });
				}

				return yield* new ExportControlUnavailable({ decision });
			});

		return { Check, Require };
	}),
);
