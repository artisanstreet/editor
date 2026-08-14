import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

const NonNegativeInt = Schema.Int.check(Schema.isGreaterThanOrEqualTo(0));

/** Durable lifecycle of one provider usage-limit interruption. */
export const UsageInterruptionState = Schema.Literals([
	"scheduled",
	"awaiting_decision",
	"launching",
	"continued",
	"cancelled",
	"failed",
]);
export type UsageInterruptionState = typeof UsageInterruptionState.Type;

/** A provider-verified model allowance that is separate and not depleted. */
export const UsageInterruptionAlternative = Schema.Struct({
	display_name: Schema.NonEmptyString,
	engine_id: Identifier,
	model_id: Schema.NonEmptyString,
	verified_at: IsoDateTime,
});
export type UsageInterruptionAlternative = typeof UsageInterruptionAlternative.Type;

/** Renderer-safe snapshot of one resumable usage-limit failure. */
export const UsageInterruption = Schema.Struct({
	affected_model_id: Schema.optional(Schema.NonEmptyString),
	alternatives: Schema.Array(UsageInterruptionAlternative).check(Schema.isMaxLength(16)),
	auto_continue: Schema.Boolean,
	cancelled_at: Schema.optional(IsoDateTime),
	continuation_command_id: Schema.optional(Identifier),
	continued_at: Schema.optional(IsoDateTime),
	created_at: IsoDateTime,
	failed_at: Schema.optional(IsoDateTime),
	interruption_id: Identifier,
	limit_id: Schema.optional(Schema.NonEmptyString),
	limit_label: Schema.optional(Schema.NonEmptyString),
	limit_scope: Schema.Literals(["shared", "model", "unknown"]),
	provider_code: Schema.optional(Schema.NonEmptyString),
	resets_at: Schema.optional(IsoDateTime),
	resume_not_before: Schema.optional(IsoDateTime),
	revision: NonNegativeInt,
	source_agent_id: Identifier,
	source_engine_id: Identifier,
	source_model_id: Schema.optional(Schema.NonEmptyString),
	source_run_id: Identifier,
	state: UsageInterruptionState,
	target_engine_id: Schema.optional(Identifier),
	target_model_id: Schema.optional(Schema.NonEmptyString),
	target_run_id: Schema.optional(Identifier),
	thread_id: Identifier,
	updated_at: IsoDateTime,
});
export type UsageInterruption = typeof UsageInterruption.Type;

/** User decision for one revision of a durable usage interruption. */
export const UsageInterruptionResolveCommand = Schema.Struct({
	action: Schema.Union([
		Schema.Struct({ enabled: Schema.Boolean, type: Schema.Literal("set_auto_continue") }),
		Schema.Struct({
			target_engine_id: Identifier,
			target_model_id: Schema.optional(Schema.NonEmptyString),
			type: Schema.Literal("continue"),
		}),
		Schema.Struct({ type: Schema.Literal("cancel") }),
	]),
	expected_revision: NonNegativeInt,
	interruption_id: Identifier,
	type: Schema.Literal("usage.interruption.resolve"),
});
export type UsageInterruptionResolveCommand = typeof UsageInterruptionResolveCommand.Type;

/** Announces the complete interruption snapshot after a durable transition. */
export const UsageInterruptionUpdatedEvent = Schema.Struct({
	interruption: UsageInterruption,
	type: Schema.Literal("usage.interruption.updated"),
});
export type UsageInterruptionUpdatedEvent = typeof UsageInterruptionUpdatedEvent.Type;
