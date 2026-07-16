import { Schema } from "effect";

import { Identifier } from "./common";

const text_encoder = new TextEncoder();

/** Defines the maximum visible character count for a capability or action label. */
export const capability_visible_label_maximum_characters = 256;

/** Defines the maximum UTF-8 byte count for a capability or action label. */
export const capability_visible_label_maximum_bytes = 512;

/** Defines the maximum UTF-8 byte count for a capability or action identifier. */
export const capability_identifier_maximum_bytes = 256;

/** Defines the maximum visible character count for a source-safe action summary. */
export const capability_safe_summary_maximum_characters = 1_024;

/** Defines the maximum UTF-8 byte count for a source-safe action summary. */
export const capability_safe_summary_maximum_bytes = 2_048;

const has_hidden_control_character = (value: string) => /[\p{Cc}\p{Cf}]/u.test(value);

/** Validates a bounded transport-safe identifier used by the canonical capability surface. */
export const CapabilityIdentifier = Identifier.check(
	Schema.makeFilter<string>((value) =>
		text_encoder.encode(value).byteLength > capability_identifier_maximum_bytes ||
		has_hidden_control_character(value)
			? `Expected a capability identifier within ${capability_identifier_maximum_bytes} UTF-8 bytes without hidden control characters`
			: undefined,
	),
);

const visible_text = (maximum_characters: number, maximum_bytes: number, description: string) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) => {
			const character_count = [...value].length;
			const byte_count = text_encoder.encode(value).byteLength;

			return value.trim().length === 0 ||
				character_count > maximum_characters ||
				byte_count > maximum_bytes ||
				has_hidden_control_character(value)
				? `Expected a non-empty ${description} within ${maximum_characters} characters and ${maximum_bytes} UTF-8 bytes without hidden control characters`
				: undefined;
		}),
	);

/** Validates the bounded label shown for a capability invocation or native action. */
export const CapabilityVisibleLabel = visible_text(
	capability_visible_label_maximum_characters,
	capability_visible_label_maximum_bytes,
	"visible label",
);

/** Validates a bounded source-safe summary without provider arguments, results, or diagnostics. */
export const CapabilitySafeSummary = visible_text(
	capability_safe_summary_maximum_characters,
	capability_safe_summary_maximum_bytes,
	"safe summary",
);

/** Classifies which canonical Artisan surface owns an observable action. */
export const CapabilitySource = Schema.Literals(["artisan", "engine", "marketplace"]);

export type CapabilitySource = typeof CapabilitySource.Type;

/** Classifies an observable action only when its effect is safely known. */
export const CapabilityEffect = Schema.Literals([
	"read",
	"durable_state",
	"workspace_mutation",
	"unknown",
]);

export type CapabilityEffect = typeof CapabilityEffect.Type;

/** Records one source-safe lifecycle update for an Artisan, engine, or marketplace capability. */
export const CapabilityInvocationUpdatedEvent = Schema.Struct({
	effect: CapabilityEffect,
	invocation_id: CapabilityIdentifier,
	label: CapabilityVisibleLabel,
	source: CapabilitySource,
	state: Schema.Literals([
		"started",
		"progress",
		"approval_required",
		"running",
		"completed",
		"failed",
		"denied",
		"outcome_unknown",
		"suspended",
	]),
	summary: Schema.optional(CapabilitySafeSummary),
	type: Schema.Literal("capability.invocation.updated"),
});

export type CapabilityInvocationUpdatedEvent = typeof CapabilityInvocationUpdatedEvent.Type;

/** Records an opaque engine-native action without exposing provider-native inputs or output. */
export const EngineNativeActionObservedEvent = Schema.Struct({
	action_id: CapabilityIdentifier,
	effect: Schema.Literal("unknown"),
	label: CapabilityVisibleLabel,
	source: Schema.Literal("engine"),
	state: Schema.Literal("observed"),
	summary: Schema.optional(CapabilitySafeSummary),
	type: Schema.Literal("engine.native_action.observed"),
});

export type EngineNativeActionObservedEvent = typeof EngineNativeActionObservedEvent.Type;
