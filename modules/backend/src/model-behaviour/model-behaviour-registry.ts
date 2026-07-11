import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	ModelBehaviourCapability,
	type ModelBehaviourActivationTiming,
	ModelBehaviourProviderCapability,
	type ModelBehaviourSettingId,
	type ModelBehaviourSupportState,
} from "@artisan/protocol";

/** Describes one provider mapping discovered by an Engine/config adapter. */
export interface ModelBehaviourProviderMapping {
	readonly activation_timing: ModelBehaviourActivationTiming;
	readonly details: string;
	readonly minimum_version?: string;
	readonly native_key?: string;
	readonly provider_id: string;
	readonly setting_id: ModelBehaviourSettingId;
	readonly state: ModelBehaviourSupportState;
}

/** Reports duplicate or malformed provider mappings during layer construction. */
export class ModelBehaviourRegistryError extends Data.TaggedError("ModelBehaviourRegistryError")<{
	readonly message: string;
}> {}

/** Owns the curated capability projection rendered by Model Behaviour settings. */
export class ModelBehaviourCapabilityRegistry extends Context.Service<
	ModelBehaviourCapabilityRegistry,
	{
		readonly Capabilities: ReadonlyArray<typeof ModelBehaviourCapability.Type>;
		readonly Find: (
			setting_id: ModelBehaviourSettingId,
		) => Option.Option<typeof ModelBehaviourCapability.Type>;
	}
>()("Artisan/ModelBehaviourCapabilityRegistry") {}

function make_auto_compaction_capability(
	provider_support: ReadonlyArray<ModelBehaviourProviderCapability>,
) {
	return Schema.decodeUnknownEffect(ModelBehaviourCapability, {
		onExcessProperty: "error",
	})({
		control: {
			kind: "integer",
			maximum: 2_000_000,
			minimum: 16_384,
			step: 128,
			unit: "tokens",
		},
		description:
			"Token threshold that triggers automatic history compaction; this does not change model context capacity.",
		display_name: "Auto-compaction trigger",
		provider_support,
		scope: "global_default",
		setting_id: "auto_compaction_trigger_tokens",
	}).pipe(
		Effect.mapError(
			() =>
				new ModelBehaviourRegistryError({
					message: "The auto-compaction capability registry entry is invalid",
				}),
		),
	);
}

/** Creates the tested Codex mapping without deriving support from a version string guess. */
export function make_codex_auto_compaction_mapping(input: {
	readonly installed_version: string;
	readonly mapping_available: boolean;
}): ModelBehaviourProviderMapping {
	return {
		activation_timing: "new_threads",
		details: input.mapping_available
			? `Codex ${input.installed_version} reads this global value when a thread starts.`
			: `Codex ${input.installed_version} did not accept the tested config mapping.`,
		native_key: "model_auto_compact_token_limit",
		provider_id: "codex",
		setting_id: "auto_compaction_trigger_tokens",
		state: input.mapping_available ? "supported" : "unavailable",
	};
}

/** Creates an unavailable mapping when the installed provider cannot be safely probed. */
export function make_unavailable_auto_compaction_mapping(
	provider_id: string,
	details: string,
	native_key?: string,
): ModelBehaviourProviderMapping {
	return {
		activation_timing: "new_threads",
		details,
		...(native_key === undefined ? {} : { native_key }),
		provider_id,
		setting_id: "auto_compaction_trigger_tokens",
		state: "unavailable",
	};
}

/** Creates a truthful unsupported mapping for providers without an equivalent control. */
export function make_unsupported_auto_compaction_mapping(
	provider_id: string,
	details: string,
): ModelBehaviourProviderMapping {
	return {
		activation_timing: "new_threads",
		details,
		provider_id,
		setting_id: "auto_compaction_trigger_tokens",
		state: "unsupported",
	};
}

/** Builds the canonical capability list from tested provider mappings. */
export function BuildModelBehaviourCapabilities(
	mappings: ReadonlyArray<ModelBehaviourProviderMapping>,
) {
	return Effect.gen(function* () {
		const identities = mappings.map(
			(mapping) => `${mapping.setting_id}:${mapping.provider_id}`,
		);

		if (new Set(identities).size !== identities.length) {
			return yield* new ModelBehaviourRegistryError({
				message: "Model Behaviour provider mappings must be unique",
			});
		}

		const provider_support = yield* Effect.forEach(mappings, (mapping) =>
			Schema.decodeUnknownEffect(ModelBehaviourProviderCapability, {
				onExcessProperty: "error",
			})({
				activation_timing: mapping.activation_timing,
				details: mapping.details,
				...(mapping.minimum_version === undefined
					? {}
					: { minimum_version: mapping.minimum_version }),
				...(mapping.native_key === undefined ? {} : { native_key: mapping.native_key }),
				provider_id: mapping.provider_id,
				state: mapping.state,
			}).pipe(
				Effect.mapError(
					() =>
						new ModelBehaviourRegistryError({
							message: `Provider mapping ${mapping.provider_id} is invalid`,
						}),
				),
			),
		);
		const capability = yield* make_auto_compaction_capability(provider_support);

		return [capability] as const;
	});
}

/** Builds a validated registry and rejects ambiguous provider ownership. */
export function make_model_behaviour_capability_registry_layer(
	mappings: ReadonlyArray<ModelBehaviourProviderMapping>,
) {
	return Layer.effect(
		ModelBehaviourCapabilityRegistry,
		Effect.gen(function* () {
			const Capabilities = yield* BuildModelBehaviourCapabilities(mappings);
			const Find = (setting_id: ModelBehaviourSettingId) =>
				Option.fromUndefinedOr(
					Capabilities.find((candidate) => candidate.setting_id === setting_id),
				);

			return { Capabilities, Find };
		}),
	);
}
