import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Option } from "effect";

import {
	type ModelBehaviourCapability,
	type ModelBehaviourSettingId,
	type ModelBehaviourValue,
} from "@artisan/protocol";

import { CodexModelBehaviourProbe, type CodexModelBehaviourProbeResult } from "./codex-probe";
import {
	codex_auto_compaction_native_key,
	patch_codex_model_behaviour,
	read_codex_model_behaviour,
} from "./codex-config";
import { ModelBehaviourConfigFiles, type ModelBehaviourConfigFileSnapshot } from "./config-files";
import {
	BuildModelBehaviourCapabilities,
	make_codex_auto_compaction_mapping,
	make_unavailable_auto_compaction_mapping,
	type ModelBehaviourProviderMapping,
} from "./registry";

/** Identifies adapter failures without exposing complete provider config content. */
export type ModelBehaviourProviderErrorCode =
	| "invalid_config"
	| "read_failed"
	| "unsupported"
	| "write_failed";

/** Reports one provider adapter failure using content-free metadata. */
export class ModelBehaviourProviderError extends Data.TaggedError("ModelBehaviourProviderError")<{
	readonly cause: unknown;
	readonly code: ModelBehaviourProviderErrorCode;
	readonly operation: "apply" | "inspect";
	readonly provider_id: string;
	readonly setting_id: ModelBehaviourSettingId;
}> {}

/** Describes the owned value and exact document revision without returning config content. */
export interface ModelBehaviourProviderObservation {
	readonly document_exists: boolean;
	readonly document_hash?: string;
	readonly modified_at?: string;
	readonly observed_hash: string;
	readonly provider_id: string;
	readonly setting_id: ModelBehaviourSettingId;
	readonly target_path: string;
	readonly value: ModelBehaviourValue;
}

/** Supplies a conditional provider write derived from a preceding observation. */
export interface ModelBehaviourProviderApplyInput {
	readonly expected_document_hash?: string;
	readonly expected_observed_hash: string;
	readonly operation_id: string;
	readonly value: ModelBehaviourValue;
}

/** Describes a provider write, no-op, or raced external edit. */
export type ModelBehaviourProviderApplyResult =
	| {
			readonly _tag: "AlreadyApplied";
			readonly observation: ModelBehaviourProviderObservation;
	  }
	| {
			readonly _tag: "Changed";
			readonly observation: ModelBehaviourProviderObservation;
	  }
	| {
			readonly _tag: "Written";
			readonly backup_path?: string;
			readonly observation: ModelBehaviourProviderObservation;
	  };

/** Owns one provider-native mapping behind the canonical behavior contract. */
export interface ModelBehaviourProviderAdapter {
	readonly Apply: (
		input: ModelBehaviourProviderApplyInput,
	) => Effect.Effect<ModelBehaviourProviderApplyResult, ModelBehaviourProviderError>;
	readonly Inspect: Effect.Effect<ModelBehaviourProviderObservation, ModelBehaviourProviderError>;
	readonly mapping: ModelBehaviourProviderMapping;
}

/** Supplies the provider mappings and capability projection used by the service and UI. */
export class ModelBehaviourProviderRegistry extends Context.Service<
	ModelBehaviourProviderRegistry,
	{
		readonly Capabilities: ReadonlyArray<ModelBehaviourCapability>;
		readonly Find: (
			provider_id: string,
			setting_id: ModelBehaviourSettingId,
		) => Option.Option<ModelBehaviourProviderAdapter>;
		readonly Providers: ReadonlyArray<ModelBehaviourProviderAdapter>;
	}
>()("Artisan/ModelBehaviourProviderRegistry") {}

/** Configures the desktop provider registry without coupling it to Electron. */
export interface DesktopModelBehaviourProviderOptions {
	readonly backups_directory: string;
	readonly codex_config_path: string;
}

interface CodexDocument {
	readonly content: string;
	readonly observation: ModelBehaviourProviderObservation;
}

function provider_error(
	code: ModelBehaviourProviderErrorCode,
	operation: ModelBehaviourProviderError["operation"],
	cause: unknown,
) {
	return new ModelBehaviourProviderError({
		cause,
		code,
		operation,
		provider_id: "codex",
		setting_id: "auto_compaction_trigger_tokens",
	});
}

function make_observation(
	value: { readonly hash: string; readonly value: ModelBehaviourValue },
	path: string,
	snapshot?: ModelBehaviourConfigFileSnapshot,
): ModelBehaviourProviderObservation {
	return {
		document_exists: snapshot !== undefined,
		...(snapshot === undefined
			? {}
			: {
					document_hash: snapshot.content_hash,
					modified_at: snapshot.modified_at,
				}),
		observed_hash: value.hash,
		provider_id: "codex",
		setting_id: "auto_compaction_trigger_tokens",
		target_path: path,
		value: value.value,
	};
}

function backup_name(operation_id: string, document_hash: string | undefined) {
	const identity = createHash("sha256")
		.update(JSON.stringify({ document_hash: document_hash ?? "absent", operation_id }))
		.digest("hex");

	return `codex-auto-compaction-${identity}.toml`;
}

/** Creates the deep Codex adapter that owns all complete-TOML handling. */
export function make_codex_model_behaviour_provider(input: {
	readonly backups_directory: string;
	readonly files: ModelBehaviourConfigFiles["Service"];
	readonly mapping: ModelBehaviourProviderMapping;
	readonly target_path: string;
}): ModelBehaviourProviderAdapter {
	const ReadDocument = Effect.gen(function* () {
		const snapshot_option = yield* input.files
			.Read(input.target_path)
			.pipe(Effect.mapError((cause) => provider_error("read_failed", "inspect", cause)));
		const snapshot = Option.getOrUndefined(snapshot_option);
		const content = snapshot?.content ?? "";
		const value = yield* read_codex_model_behaviour(content).pipe(
			Effect.mapError((cause) => provider_error("invalid_config", "inspect", cause)),
		);

		return {
			content,
			observation: make_observation(value, input.target_path, snapshot),
		} satisfies CodexDocument;
	});
	const Inspect = ReadDocument.pipe(Effect.map((document) => document.observation));
	const Apply = (request: ModelBehaviourProviderApplyInput) =>
		Effect.gen(function* () {
			const current = yield* ReadDocument;
			const observation = current.observation;

			if (
				observation.observed_hash !== request.expected_observed_hash ||
				observation.document_hash !== request.expected_document_hash
			) {
				return { _tag: "Changed" as const, observation };
			}

			const patched = yield* patch_codex_model_behaviour(current.content, request.value).pipe(
				Effect.mapError((cause) => provider_error("invalid_config", "apply", cause)),
			);

			if (patched.value.hash === observation.observed_hash) {
				return { _tag: "AlreadyApplied" as const, observation };
			}

			const publication = yield* input.files
				.ReplaceAtomic({
					backups_directory: input.backups_directory,
					backup_name: backup_name(request.operation_id, observation.document_hash),
					content: patched.content,
					...(observation.document_hash === undefined
						? {}
						: { expected_content_hash: observation.document_hash }),
					path: input.target_path,
				})
				.pipe(Effect.mapError((cause) => provider_error("write_failed", "apply", cause)));

			if (publication._tag === "Changed") {
				const changed = yield* ReadDocument;

				return { _tag: "Changed" as const, observation: changed.observation };
			}

			const written = yield* read_codex_model_behaviour(publication.content).pipe(
				Effect.mapError((cause) => provider_error("invalid_config", "apply", cause)),
			);

			if (written.hash !== patched.value.hash) {
				return {
					_tag: "Changed" as const,
					observation: make_observation(written, input.target_path, publication),
				};
			}

			return {
				_tag: "Written" as const,
				...(publication.backup_path === undefined
					? {}
					: { backup_path: publication.backup_path }),
				observation: make_observation(written, input.target_path, publication),
			};
		});

	return { Apply, Inspect, mapping: input.mapping };
}

/** Creates a non-writing adapter for unsupported or unavailable providers. */
export function make_inactive_model_behaviour_provider(
	mapping: ModelBehaviourProviderMapping,
): ModelBehaviourProviderAdapter {
	const failure = new ModelBehaviourProviderError({
		cause: new Error(mapping.details),
		code: "unsupported",
		operation: "inspect",
		provider_id: mapping.provider_id,
		setting_id: mapping.setting_id,
	});

	return {
		Apply: () =>
			Effect.fail(
				new ModelBehaviourProviderError({
					...failure,
					operation: "apply",
				}),
			),
		Inspect: Effect.fail(failure),
		mapping,
	};
}

function BuildProviderRegistry(providers: ReadonlyArray<ModelBehaviourProviderAdapter>) {
	return Effect.gen(function* () {
		const Capabilities = yield* BuildModelBehaviourCapabilities(
			providers.map((provider) => provider.mapping),
		);
		const Find = (provider_id: string, setting_id: ModelBehaviourSettingId) =>
			Option.fromUndefinedOr(
				providers.find(
					(provider) =>
						provider.mapping.provider_id === provider_id &&
						provider.mapping.setting_id === setting_id,
				),
			);

		return { Capabilities, Find, Providers: providers };
	});
}

/** Builds a tested provider registry from explicit adapters. */
export function make_model_behaviour_provider_registry_layer(
	providers: ReadonlyArray<ModelBehaviourProviderAdapter>,
) {
	return Layer.effect(ModelBehaviourProviderRegistry, BuildProviderRegistry(providers));
}

/** Provides the canonical control with no installed provider support. */
export const EmptyModelBehaviourProviderRegistryLive = Layer.effect(
	ModelBehaviourProviderRegistry,
	BuildProviderRegistry([]),
);

function mapping_from_probe(probe: CodexModelBehaviourProbeResult) {
	return probe.type === "available"
		? make_codex_auto_compaction_mapping(probe)
		: make_unavailable_auto_compaction_mapping(
				"codex",
				"Codex could not be feature-probed, so Artisan left its config untouched.",
				codex_auto_compaction_native_key,
			);
}

/** Builds the desktop Codex provider registry after probing the installed CLI. */
export function make_desktop_model_behaviour_provider_registry_layer(
	options: DesktopModelBehaviourProviderOptions,
) {
	return Layer.effect(
		ModelBehaviourProviderRegistry,
		Effect.gen(function* () {
			const probe = yield* CodexModelBehaviourProbe;
			const files = yield* ModelBehaviourConfigFiles;
			const codex_mapping = mapping_from_probe(yield* probe.Probe);
			const providers = [
				codex_mapping.state === "supported" || codex_mapping.state === "experimental"
					? make_codex_model_behaviour_provider({
							backups_directory: options.backups_directory,
							files,
							mapping: codex_mapping,
							target_path: options.codex_config_path,
						})
					: make_inactive_model_behaviour_provider(codex_mapping),
			];

			return yield* BuildProviderRegistry(providers);
		}),
	);
}
