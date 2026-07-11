import { Schema } from "effect";

import { Identifier, IsoDateTime, StreamSequence } from "./common";
import { GuidanceHash } from "./guidance";

/** Identifies the curated, provider-neutral model controls owned by Artisan V1. */
export const ModelBehaviourSettingId = Schema.Literal("auto_compaction_trigger_tokens");

export type ModelBehaviourSettingId = typeof ModelBehaviourSettingId.Type;

/** Bounds an explicit auto-compaction trigger without presenting it as context capacity. */
export const AutoCompactionTriggerTokens = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(16_384),
	Schema.isLessThanOrEqualTo(2_000_000),
);

/** Represents either an Artisan-owned value or the provider/model default. */
export const ModelBehaviourValue = Schema.Union([
	Schema.Struct({ type: Schema.Literal("provider_default") }),
	Schema.Struct({
		type: Schema.Literal("integer"),
		value: AutoCompactionTriggerTokens,
	}),
]);

export type ModelBehaviourValue = typeof ModelBehaviourValue.Type;

/** Describes whether one provider can apply a canonical control. */
export const ModelBehaviourSupportState = Schema.Literals([
	"supported",
	"experimental",
	"runtime_only",
	"unsupported",
	"unavailable",
]);

export type ModelBehaviourSupportState = typeof ModelBehaviourSupportState.Type;

/** Identifies when a changed behavior becomes active for one provider. */
export const ModelBehaviourActivationTiming = Schema.Literals([
	"immediate",
	"next_turn",
	"new_threads",
	"restart_required",
]);

export type ModelBehaviourActivationTiming = typeof ModelBehaviourActivationTiming.Type;

/** Projects one provider's version-gated mapping without making native keys canonical. */
export const ModelBehaviourProviderCapability = Schema.Struct({
	activation_timing: ModelBehaviourActivationTiming,
	details: Schema.NonEmptyString,
	minimum_version: Schema.optional(Schema.NonEmptyString),
	native_key: Schema.optional(Schema.NonEmptyString),
	provider_id: Identifier,
	state: ModelBehaviourSupportState,
});

export type ModelBehaviourProviderCapability = typeof ModelBehaviourProviderCapability.Type;

/** Supplies the numeric editor contract for one curated setting. */
export const ModelBehaviourIntegerControl = Schema.Struct({
	kind: Schema.Literal("integer"),
	maximum: AutoCompactionTriggerTokens,
	minimum: AutoCompactionTriggerTokens,
	step: Schema.Int.check(Schema.isGreaterThan(0)),
	unit: Schema.Literal("tokens"),
}).check(
	Schema.makeFilter((control) => {
		if (control.minimum > control.maximum) {
			return "Expected the Model Behaviour control minimum not to exceed its maximum";
		}

		return (control.maximum - control.minimum) % control.step === 0
			? undefined
			: "Expected the Model Behaviour control range to be divisible by its step";
	}),
);

export type ModelBehaviourIntegerControl = typeof ModelBehaviourIntegerControl.Type;

/** Describes one canonical setting rendered by the Model Behaviour tab. */
export const ModelBehaviourCapability = Schema.Struct({
	control: ModelBehaviourIntegerControl,
	description: Schema.NonEmptyString,
	display_name: Schema.NonEmptyString,
	provider_support: Schema.Array(ModelBehaviourProviderCapability),
	scope: Schema.Literal("global_default"),
	setting_id: ModelBehaviourSettingId,
});

export type ModelBehaviourCapability = typeof ModelBehaviourCapability.Type;

/** Projects the current canonical value and its durable version. */
export const ModelBehaviourSetting = Schema.Struct({
	setting_id: ModelBehaviourSettingId,
	updated_at: IsoDateTime,
	value: ModelBehaviourValue,
	version: StreamSequence,
});

export type ModelBehaviourSetting = typeof ModelBehaviourSetting.Type;

/** Describes one provider config reconciliation without ingesting its whole file. */
export const ModelBehaviourProviderState = Schema.Struct({
	applied_hash: Schema.optional(GuidanceHash),
	backup_path: Schema.optional(Schema.NonEmptyString),
	ignored_drift_hash: Schema.optional(GuidanceHash),
	last_error_code: Schema.optional(Identifier),
	native_key: Schema.optional(Schema.NonEmptyString),
	observed_hash: Schema.optional(GuidanceHash),
	provider_id: Identifier,
	setting_id: ModelBehaviourSettingId,
	status: Schema.Literals([
		"synced",
		"provider_default",
		"drift_detected",
		"drift_ignored",
		"sync_failed",
		"unsupported",
		"version_unavailable",
		"runtime_only",
	]),
	target_path: Schema.optional(Schema.NonEmptyString),
	updated_at: IsoDateTime,
});

export type ModelBehaviourProviderState = typeof ModelBehaviourProviderState.Type;

/** Returns the versioned registry, canonical values, and provider reconciliation state. */
export const ModelBehaviourSnapshot = Schema.Struct({
	capabilities: Schema.Array(ModelBehaviourCapability),
	providers: Schema.Array(ModelBehaviourProviderState),
	registry_version: Schema.Literal(1),
	settings: Schema.Array(ModelBehaviourSetting),
});

export type ModelBehaviourSnapshot = typeof ModelBehaviourSnapshot.Type;

/** Replaces one canonical global behavior through an idempotent operation. */
export const ModelBehaviourUpdateRequest = Schema.Struct({
	setting_id: ModelBehaviourSettingId,
	value: ModelBehaviourValue,
});

export type ModelBehaviourUpdateRequest = typeof ModelBehaviourUpdateRequest.Type;

/** Resolves one exact provider value observed after an external config edit. */
export const ModelBehaviourDriftResolutionRequest = Schema.Struct({
	action: Schema.Literals(["ignore", "import", "overwrite"]),
	observed_hash: GuidanceHash,
	provider_id: Identifier,
	setting_id: ModelBehaviourSettingId,
});

export type ModelBehaviourDriftResolutionRequest = typeof ModelBehaviourDriftResolutionRequest.Type;

/** Retries the opinionated mapping for one provider and canonical setting. */
export const ModelBehaviourRetryRequest = Schema.Struct({
	provider_id: Identifier,
	setting_id: ModelBehaviourSettingId,
});

export type ModelBehaviourRetryRequest = typeof ModelBehaviourRetryRequest.Type;

/** Records one canonical behavior update in the settings event stream. */
export const ModelBehaviourSettingUpdatedEvent = Schema.Struct({
	setting_id: ModelBehaviourSettingId,
	type: Schema.Literal("model_behaviour.setting.updated"),
	value: ModelBehaviourValue,
	version: StreamSequence,
});

export type ModelBehaviourSettingUpdatedEvent = typeof ModelBehaviourSettingUpdatedEvent.Type;

/** Records provider reconciliation without persisting unrelated config content. */
export const ModelBehaviourProviderReconciledEvent = Schema.Struct({
	applied_hash: Schema.optional(GuidanceHash),
	ignored_drift_hash: Schema.optional(GuidanceHash),
	last_error_code: Schema.optional(Identifier),
	observed_hash: Schema.optional(GuidanceHash),
	provider_id: Identifier,
	setting_id: ModelBehaviourSettingId,
	status: ModelBehaviourProviderState.fields.status,
	type: Schema.Literal("model_behaviour.provider.reconciled"),
});

export type ModelBehaviourProviderReconciledEvent =
	typeof ModelBehaviourProviderReconciledEvent.Type;
