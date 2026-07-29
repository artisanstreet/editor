import { Schema } from "effect";

export const HarnessId = Schema.Literals(["codex", "claude", "grok", "cursor"]);
export type HarnessId = typeof HarnessId.Type;

/** Provider identifiers are open because account-discovered harnesses add labs independently. */
export const ProviderId = Schema.NonEmptyString;
export type ProviderId = typeof ProviderId.Type;

/** Artisan's ordered presentation vocabulary. Adapters retain their native value separately. */
export const ThinkingLevel = Schema.Literals(["light", "medium", "high", "xhigh", "max"]);
export type ThinkingLevel = typeof ThinkingLevel.Type;

export const ThinkingEconomics = Schema.Literals(["standard", "diminishing-returns"]);
export type ThinkingEconomics = typeof ThinkingEconomics.Type;

export const thinking_level_order = Schema.decodeUnknownSync(Schema.Array(ThinkingLevel))([
	"light",
	"medium",
	"high",
	"xhigh",
	"max",
]);

const thinking_rank = new Map(thinking_level_order.map((level, index) => [level, index]));

export const ThinkingOption = Schema.Struct({
	economics: ThinkingEconomics,
	id: ThinkingLevel,
	native_value: Schema.NonEmptyString,
});
export type ThinkingOption = typeof ThinkingOption.Type;

export const SupportedThinkingCapability = Schema.Struct({
	availability: Schema.Literal("supported"),
	default: ThinkingLevel,
	options: Schema.NonEmptyArray(ThinkingOption),
}).check(
	Schema.makeFilter((capability) => {
		const issues: Array<Schema.FilterIssue> = [];
		const ids = capability.options.map((option) => option.id);
		if (new Set(ids).size !== ids.length) {
			issues.push({ path: ["options"], issue: "thinking option IDs must be unique" });
		}
		const native_values = capability.options.map((option) => option.native_value);
		if (new Set(native_values).size !== native_values.length) {
			issues.push({ path: ["options"], issue: "native thinking values must be unique" });
		}
		if (!ids.includes(capability.default)) {
			issues.push({ path: ["default"], issue: "default thinking level must be supported" });
		}
		for (let index = 1; index < ids.length; index += 1) {
			const previous_id = ids[index - 1];
			const current_id = ids[index];
			if (previous_id === undefined || current_id === undefined) {
				issues.push({ path: ["options", index], issue: "thinking option is missing" });
				break;
			}
			const previous = thinking_rank.get(previous_id);
			const current = thinking_rank.get(current_id);
			if (previous === undefined || current === undefined || previous >= current) {
				issues.push({
					path: ["options", index],
					issue: "thinking options must be ordered bottom-up",
				});
				break;
			}
		}
		return issues;
	}),
);

export const ThinkingCapability = Schema.Union([
	Schema.Struct({ availability: Schema.Literal("unavailable") }),
	SupportedThinkingCapability,
	Schema.Struct({
		availability: Schema.Literal("native"),
		description: Schema.String,
	}),
]);
export type ThinkingCapability = typeof ThinkingCapability.Type;

export const ContextWindowOption = Schema.Struct({
	id: Schema.NonEmptyString,
	label: Schema.NonEmptyString,
	/** Appended verbatim to the native model id; empty for the base window. */
	native_suffix: Schema.String,
	tokens: Schema.Int.check(Schema.isGreaterThan(0)),
});
export type ContextWindowOption = typeof ContextWindowOption.Type;

/**
 * A model's context-window choice. Models without the capability omit it from
 * `ModelCapabilities`; the window is whatever the harness serves.
 */
export const ContextWindowCapability = Schema.Struct({
	availability: Schema.Literal("configurable"),
	default: Schema.NonEmptyString,
	options: Schema.NonEmptyArray(ContextWindowOption),
}).check(
	Schema.makeFilter((capability) => {
		const issues: Array<Schema.FilterIssue> = [];
		const ids = capability.options.map((option) => option.id);
		if (new Set(ids).size !== ids.length) {
			issues.push({ path: ["options"], issue: "context window option IDs must be unique" });
		}
		const suffixes = capability.options.map((option) => option.native_suffix);
		if (new Set(suffixes).size !== suffixes.length) {
			issues.push({ path: ["options"], issue: "native context suffixes must be unique" });
		}
		if (!ids.includes(capability.default)) {
			issues.push({ path: ["default"], issue: "default context window must be an option" });
		}
		return issues;
	}),
);
export type ContextWindowCapability = typeof ContextWindowCapability.Type;

export const Disabled = Schema.Struct({
	reason: Schema.NonEmptyString,
});
export type Disabled = typeof Disabled.Type;

export const SpeedOption = Schema.Struct({
	availability: Schema.Literals(["always", "dynamic"]),
	consumption_basis: Schema.Literals(["standard", "chatgpt-credits", "usage-credit-price"]),
	/** Null when pricing is unpublished or input/output ratios are asymmetric. */
	consumption_multiplier: Schema.NullOr(Schema.Number.check(Schema.isGreaterThanOrEqualTo(1))),
	input_consumption_multiplier: Schema.optional(
		Schema.NullOr(Schema.Number.check(Schema.isGreaterThanOrEqualTo(1))),
	),
	output_consumption_multiplier: Schema.optional(
		Schema.NullOr(Schema.Number.check(Schema.isGreaterThanOrEqualTo(1))),
	),
	default: Schema.Boolean,
	description: Schema.String,
	disabled: Schema.optional(Disabled),
	id: Schema.NonEmptyString,
	label: Schema.NonEmptyString,
	native_value: Schema.NonEmptyString,
	source_url: Schema.NonEmptyString,
	/** Null when the provider markets a faster tier without publishing a numeric ratio. */
	speed_multiplier: Schema.NullOr(Schema.Number.check(Schema.isGreaterThanOrEqualTo(1))),
	verified_at: Schema.NonEmptyString,
}).check(
	Schema.makeFilter((option) => {
		const input = option.input_consumption_multiplier;
		const output = option.output_consumption_multiplier;
		if (input === undefined || output === undefined) {
			return [];
		}
		if (input === null || output === null || input !== output) {
			return option.consumption_multiplier === null
				? []
				: [
						{
							path: ["consumption_multiplier"],
							issue: "aggregate multiplier must be null when component ratios differ or are unknown",
						},
					];
		}
		return option.consumption_multiplier === input
			? []
			: [
					{
						path: ["consumption_multiplier"],
						issue: "aggregate multiplier must match equal input and output ratios",
					},
				];
	}),
);
export type SpeedOption = typeof SpeedOption.Type;

export const SpeedOptions = Schema.NonEmptyArray(SpeedOption).check(
	Schema.makeFilter((options) => {
		const issues: Array<Schema.FilterIssue> = [];
		const ids = options.map((option) => option.id);
		const native_values = options.map((option) => option.native_value);
		if (new Set(ids).size !== ids.length) {
			issues.push({ path: [], issue: "speed option IDs must be unique" });
		}
		if (new Set(native_values).size !== native_values.length) {
			issues.push({ path: [], issue: "native speed values must be unique" });
		}
		if (options.filter((option) => option.default).length !== 1) {
			issues.push({ path: [], issue: "speed options must declare exactly one default" });
		}
		return issues;
	}),
);
export type SpeedOptions = typeof SpeedOptions.Type;

/** Artisan's ordered permission vocabulary; harnesses may expose a sparse subset. */
export const PermissionLevel = Schema.Literals([
	"restricted",
	"supervised",
	"trusted",
	"autonomous",
	"unrestricted",
]);
export type PermissionLevel = typeof PermissionLevel.Type;

export const permission_level_order = Schema.decodeUnknownSync(Schema.Array(PermissionLevel))([
	"restricted",
	"supervised",
	"trusted",
	"autonomous",
	"unrestricted",
]);

const permission_rank = new Map(permission_level_order.map((level, index) => [level, index]));

export const PermissionOption = Schema.Struct({
	approval_behavior: Schema.Literals(["prompts", "classifier", "none"]),
	availability: Schema.Literals(["always", "dynamic"]),
	description: Schema.NonEmptyString,
	edit_scope: Schema.Literals(["none", "workspace", "host"]),
	id: PermissionLevel,
	label: Schema.NonEmptyString,
	native_value: Schema.NonEmptyString,
	safety_boundary: Schema.Literals(["plan", "sandbox", "rules", "bypassed"]),
});
export type PermissionOption = typeof PermissionOption.Type;

export const PermissionCapability = Schema.Struct({
	default: PermissionLevel,
	options: Schema.NonEmptyArray(PermissionOption),
}).check(
	Schema.makeFilter((capability) => {
		const issues: Array<Schema.FilterIssue> = [];
		const ids = capability.options.map((option) => option.id);
		if (new Set(ids).size !== ids.length) {
			issues.push({ path: ["options"], issue: "permission option IDs must be unique" });
		}
		const native_values = capability.options.map((option) => option.native_value);
		if (new Set(native_values).size !== native_values.length) {
			issues.push({
				path: ["options"],
				issue: "native permission values must be unique",
			});
		}
		if (!ids.includes(capability.default)) {
			issues.push({ path: ["default"], issue: "default permission level must be supported" });
		}
		for (let index = 1; index < ids.length; index += 1) {
			const previous = permission_rank.get(ids[index - 1] as PermissionLevel);
			const current = permission_rank.get(ids[index] as PermissionLevel);
			if (previous === undefined || current === undefined || previous >= current) {
				issues.push({
					path: ["options", index],
					issue: "permission options must be ordered from least to most autonomous",
				});
				break;
			}
		}
		return issues;
	}),
);
export type PermissionCapability = typeof PermissionCapability.Type;

export const ModelCapabilities = Schema.Struct({
	context_window: Schema.optional(ContextWindowCapability),
	image_input: Schema.Boolean,
	local_tools: Schema.Boolean,
	mcp: Schema.Boolean,
	speed_options: SpeedOptions,
	thinking: ThinkingCapability,
	web_search: Schema.Boolean,
});
export type ModelCapabilities = typeof ModelCapabilities.Type;

export const ProviderDefinition = Schema.Struct({ id: ProviderId, label: Schema.String });
export type ProviderDefinition = typeof ProviderDefinition.Type;

export const GatewayDefinition = Schema.Struct({
	id: Schema.NonEmptyString,
	kind: Schema.Literals(["managed", "provider-direct"]),
	label: Schema.NonEmptyString,
});
export type GatewayDefinition = typeof GatewayDefinition.Type;

export const HarnessDefinition = Schema.Struct({
	id: HarnessId,
	gateways: Schema.Array(GatewayDefinition),
	label: Schema.String,
	permissions: PermissionCapability,
});
export type HarnessDefinition = typeof HarnessDefinition.Type;

export const ModelDefinition = Schema.Struct({
	capabilities: ModelCapabilities,
	/**
	 * One-line vendor copy sourced from the harness's own model picker or the
	 * provider's docs. Absent for dynamically discovered models (for example
	 * Cursor account inventory), which have no official description to quote.
	 */
	description: Schema.optional(Schema.NonEmptyString),
	disabled: Schema.optional(Disabled),
	harness: HarnessId,
	id: Schema.String,
	name: Schema.String,
	native_model_id: Schema.NonEmptyString,
	provider: ProviderId,
	routing: Schema.Union([
		Schema.Struct({ kind: Schema.Literal("default") }),
		Schema.Struct({ gateway_id: Schema.NonEmptyString, kind: Schema.Literal("gateway") }),
	]),
	status: Schema.Literals(["curated", "prototype"]),
});
export type ModelDefinition = typeof ModelDefinition.Type;

export const ModelManifest = Schema.Struct({
	harnesses: Schema.Array(HarnessDefinition),
	models: Schema.Array(ModelDefinition),
	providers: Schema.Array(ProviderDefinition),
	revision: Schema.String,
}).check(
	Schema.makeFilter((manifest) => {
		const issues: Array<Schema.FilterIssue> = [];
		const provider_ids = manifest.providers.map((provider) => provider.id);
		const harness_ids = manifest.harnesses.map((harness) => harness.id);
		const model_ids = manifest.models.map((model) => model.id);
		for (const [path, ids] of [
			["providers", provider_ids],
			["harnesses", harness_ids],
			["models", model_ids],
		] as const) {
			if (new Set(ids).size !== ids.length) {
				issues.push({ path: [path], issue: `${path} must have unique IDs` });
			}
		}
		for (const [index, harness] of manifest.harnesses.entries()) {
			const gateway_ids = harness.gateways.map((gateway) => gateway.id);
			if (new Set(gateway_ids).size !== gateway_ids.length) {
				issues.push({
					path: ["harnesses", index, "gateways"],
					issue: "gateway IDs must be unique within a harness",
				});
			}
		}
		for (const [index, model] of manifest.models.entries()) {
			if (!provider_ids.includes(model.provider)) {
				issues.push({ path: ["models", index, "provider"], issue: "unknown provider" });
			}
			if (!harness_ids.includes(model.harness)) {
				issues.push({ path: ["models", index, "harness"], issue: "unknown harness" });
			}
			const harness = manifest.harnesses.find((candidate) => candidate.id === model.harness);
			if (model.routing.kind === "gateway") {
				const gateway_id = model.routing.gateway_id;
				if (!harness?.gateways.some((gateway) => gateway.id === gateway_id)) {
					issues.push({
						path: ["models", index, "routing"],
						issue: "unknown harness gateway",
					});
				}
			} else if (harness !== undefined && harness.gateways.length > 0) {
				issues.push({
					path: ["models", index, "routing"],
					issue: "models on gateway-routed harnesses must declare a gateway",
				});
			}
		}
		return issues;
	}),
);
export type ModelManifest = typeof ModelManifest.Type;
