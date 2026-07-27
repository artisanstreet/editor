import { Schema } from "effect";

import {
	ModelDefinition,
	ModelManifest,
	ProviderDefinition,
	SpeedOption,
	type ProviderId,
} from "./schema";

export const CursorAccountModelId = Schema.NonEmptyString;
export type CursorAccountModelId = typeof CursorAccountModelId.Type;

export type CursorAccountModelInput =
	| CursorAccountModelId
	| {
			readonly native_model_id: CursorAccountModelId;
			readonly provider?: ProviderDefinition;
	  };

export const CursorAccountCatalog = Schema.Struct({
	models: Schema.Array(ModelDefinition),
	providers: Schema.Array(ProviderDefinition),
});
export type CursorAccountCatalog = typeof CursorAccountCatalog.Type;

const provider_rules: ReadonlyArray<{
	readonly id: ProviderId;
	readonly label: string;
	readonly pattern: RegExp;
}> = [
	{ id: "openai", label: "OpenAI", pattern: /^(?:gpt|o\d|chatgpt)-/i },
	{ id: "anthropic", label: "Anthropic", pattern: /^claude-/i },
	{ id: "google", label: "Google", pattern: /^gemini-/i },
	{ id: "xai", label: "xAI", pattern: /^grok-/i },
	{ id: "moonshot", label: "Moonshot AI", pattern: /^kimi-/i },
	{ id: "deepseek", label: "DeepSeek", pattern: /^deepseek-/i },
	{ id: "zhipu", label: "Zhipu AI", pattern: /^glm-/i },
	{ id: "minimax", label: "MiniMax", pattern: /^minimax-/i },
	{ id: "mistral", label: "Mistral AI", pattern: /^(?:codestral|mistral)-/i },
	{ id: "meta", label: "Meta", pattern: /^llama-/i },
	{
		id: "cursor",
		label: "Cursor",
		pattern: /^(?:auto|composer-|cursor-)/i,
	},
];

const title_case_model_id = (native_model_id: string) =>
	native_model_id
		.split("-")
		.map((part) => {
			const upper = part.toUpperCase();
			return /^(?:AI|GPT|GLM|KIMI|LLAMA|MISTRAL|OPUS|R1|V\d)$/i.test(part)
				? upper
				: `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`;
		})
		.join(" ");

const infer_provider = (native_model_id: string) =>
	provider_rules.find(({ pattern }) => pattern.test(native_model_id)) ?? {
		id: "unknown",
		label: "Unknown provider",
	};

const infer_thinking = (native_model_id: string) => ({
	availability: "native" as const,
	description: `Cursor exposes ${native_model_id} as a complete native configuration. Any encoded reasoning level is part of its model ID, not a separately documented CLI control.`,
});

const infer_speed = (native_model_id: string) => {
	const is_fast = /-fast$/i.test(native_model_id);
	return [
		Schema.decodeUnknownSync(SpeedOption)({
			availability: "dynamic",
			consumption_basis: "usage-credit-price",
			consumption_multiplier: null,
			input_consumption_multiplier: null,
			output_consumption_multiplier: null,
			default: true,
			description: `${title_case_model_id(native_model_id)} uses its Cursor account price for ${
				is_fast ? "the selected Fast configuration" : "provider-native speed"
			}. Fast mode is ${
				is_fast
					? "selected in this native model configuration"
					: "not encoded in this discovered configuration"
			}; consult Cursor's live model catalog for current pricing and availability.`,
			id: is_fast ? "fast" : "standard",
			label: is_fast ? "Fast" : "Native",
			native_value: is_fast ? "fast" : "standard",
			source_url: "https://cursor.com/docs/models",
			speed_multiplier: null,
			verified_at: "2026-07-27",
		}),
	];
};

/**
 * Converts the complete account-scoped flat Cursor CLI inventory into Artisan models.
 * Native IDs remain authoritative because Cursor silently falls back for invalid aliases.
 */
export const make_cursor_account_catalog = (
	inputs: ReadonlyArray<CursorAccountModelInput>,
): CursorAccountCatalog => {
	const input_by_id = new Map<string, Exclude<CursorAccountModelInput, string>>();
	for (const input of inputs) {
		const normalized = typeof input === "string" ? { native_model_id: input } : input;
		const existing = input_by_id.get(normalized.native_model_id);
		if (existing?.provider === undefined || normalized.provider !== undefined) {
			input_by_id.set(normalized.native_model_id, normalized);
		}
	}
	const unique_inputs = [...input_by_id.values()].sort((left, right) =>
		left.native_model_id.localeCompare(right.native_model_id),
	);
	const provider_by_id = new Map<string, ProviderDefinition>();
	const models = unique_inputs.map(({ native_model_id, provider: supplied_provider }) => {
		const provider = supplied_provider ?? infer_provider(native_model_id);
		provider_by_id.set(provider.id, Schema.decodeUnknownSync(ProviderDefinition)(provider));

		return Schema.decodeUnknownSync(ModelDefinition)({
			capabilities: {
				image_input: false,
				local_tools: true,
				mcp: true,
				speed_options: infer_speed(native_model_id),
				thinking: infer_thinking(native_model_id),
				web_search: false,
			},
			harness: "cursor",
			id: `cursor-account-${native_model_id}`,
			name: title_case_model_id(native_model_id),
			native_model_id,
			provider: provider.id,
			routing: { kind: "default" },
			status: "prototype",
		});
	});

	return Schema.decodeUnknownSync(CursorAccountCatalog)({
		models,
		providers: [...provider_by_id.values()].sort((left, right) =>
			left.label.localeCompare(right.label),
		),
	});
};

/**
 * Replaces stale account-discovered Cursor entries while retaining richer curated records.
 */
export const merge_cursor_account_catalog = (
	manifest: ModelManifest,
	inputs: ReadonlyArray<CursorAccountModelInput>,
): ModelManifest => {
	const discovered = make_cursor_account_catalog(inputs);
	const discovered_native_ids = new Set(discovered.models.map((model) => model.native_model_id));
	const curated_native_ids = new Set(
		manifest.models
			.filter(
				(model) => model.harness === "cursor" && !model.id.startsWith("cursor-account-"),
			)
			.map((model) => model.native_model_id),
	);
	const retained_models = manifest.models
		.filter((model) => model.harness !== "cursor" || !model.id.startsWith("cursor-account-"))
		.map((model) => {
			if (model.harness !== "cursor") {
				return model;
			}
			const matching_ids = [...discovered_native_ids].filter(
				(native_model_id) =>
					native_model_id === model.native_model_id ||
					native_model_id.startsWith(`${model.native_model_id}-`),
			);
			const { disabled: _model_disabled, ...enabled_model } = model;
			const speed_options = model.capabilities.speed_options.map((option) => {
				const option_available =
					option.id !== "fast" ||
					matching_ids.some((native_model_id) => /-fast$/i.test(native_model_id));
				const { disabled: _option_disabled, ...enabled_option } = option;
				return option_available
					? enabled_option
					: {
							...enabled_option,
							disabled: {
								reason: "Cursor did not return a Fast configuration for this authenticated account.",
							},
						};
			});
			const enabled_default = speed_options.find(
				(option) => option.default && !("disabled" in option),
			);
			const fallback_default = speed_options.find((option) => !("disabled" in option));
			const default_id = enabled_default?.id ?? fallback_default?.id;
			const capabilities = {
				...model.capabilities,
				speed_options: speed_options.map((option) => ({
					...option,
					default: option.id === default_id,
				})),
			};

			return matching_ids.length > 0
				? { ...enabled_model, capabilities }
				: {
						...enabled_model,
						capabilities,
						disabled: {
							reason: "Cursor did not return this model for the authenticated account, region, or organization policy.",
						},
					};
		});
	const available_providers = new Map(
		[...manifest.providers, ...discovered.providers].map((provider) => [provider.id, provider]),
	);
	const models = [
		...retained_models,
		...discovered.models.filter((model) => !curated_native_ids.has(model.native_model_id)),
	];
	const referenced_provider_ids = new Set(models.map((model) => model.provider));

	return Schema.decodeUnknownSync(ModelManifest)({
		...manifest,
		models,
		providers: [...available_providers.values()].filter((provider) =>
			referenced_provider_ids.has(provider.id),
		),
		revision: `${manifest.revision.replace(/(?:\+cursor-account)+$/, "")}+cursor-account`,
	});
};
