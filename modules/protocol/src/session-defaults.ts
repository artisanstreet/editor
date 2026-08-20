import { Effect, Schema, SchemaGetter } from "effect";

import { Identifier } from "./common";

/**
 * Defaults a new thread inherits, owned by Forge rather than any one browser.
 *
 * Split by what each setting is about. `permission` is a statement about how
 * much the agent may do in your workspace, so it is shared across every model
 * and engine and stored once. Reasoning effort, speed tier, and context window
 * describe a particular catalog model — not every model offers `max` or Fast,
 * and context suffixes are native to their harness — so they are stored per
 * model and never coerced onto a model that cannot express them.
 *
 * @since 0.8.0
 */

const session_defaults_maximum_models = 512;

/** Selects the thread's own current model as its compaction summarizer. */
export const inherited_compaction_model = "inherited";

/** Stable public ids for the curated agent-name banks bundled by the data module. */
export const AgentNameDatasetIds = ["norwegian", "british"] as const;
export type AgentNameDatasetId = (typeof AgentNameDatasetIds)[number];
export const DefaultAgentNameDatasetId: AgentNameDatasetId = "norwegian";
export const AgentNameDatasets = [
	{ description: "Norwegian feminine given names.", id: "norwegian", label: "Norwegian" },
	{ description: "British feminine given names.", id: "british", label: "British" },
] as const;

export const AgentNameDataset = Schema.Literals(["norwegian", "british", "playful"]).pipe(
	Schema.decodeTo(Schema.Literals(AgentNameDatasetIds), {
		decode: SchemaGetter.transform((dataset) => (dataset === "playful" ? "british" : dataset)),
		encode: SchemaGetter.transform((dataset) => dataset),
	}),
	Schema.withDecodingDefault(Effect.succeed(DefaultAgentNameDatasetId)),
);

export type AgentNameDataset = typeof AgentNameDataset.Type;

/** Records the controls one catalog model was last configured with. */
export const SessionModelDefaults = Schema.Struct({
	/** The native context-window suffix, absent for the model's base window. */
	context_window: Schema.optional(Schema.NonEmptyString),
	model_id: Schema.NonEmptyString,
	reasoning_effort: Schema.optional(
		Schema.Literals(["low", "medium", "high", "xhigh", "max", "ultra"]),
	),
	/** The provider-native speed tier, absent when the model uses its catalog default. */
	service_tier: Schema.optional(Schema.NonEmptyString),
});

export type SessionModelDefaults = typeof SessionModelDefaults.Type;

/**
 * Patches one model's defaults. An omitted field is left unchanged; `null`
 * explicitly clears the saved override and restores the catalog default.
 */
export const SessionModelDefaultsUpdate = Schema.Struct({
	context_window: Schema.optional(Schema.NullOr(Schema.NonEmptyString)),
	model_id: Schema.NonEmptyString,
	reasoning_effort: Schema.optional(
		Schema.NullOr(Schema.Literals(["low", "medium", "high", "xhigh", "max", "ultra"])),
	),
	/** `null` restores the selected model's catalog speed tier. */
	service_tier: Schema.optional(Schema.NullOr(Schema.NonEmptyString)),
});

export type SessionModelDefaultsUpdate = typeof SessionModelDefaultsUpdate.Type;

/** Projects every default a draft reads when it opens. */
export const SessionDefaults = Schema.Struct({
	auto_continue_usage_limits: Schema.Boolean.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed(true)),
	),
	agent_name_dataset: Schema.optional(AgentNameDataset),
	/**
	 * How handoff compaction picks its summarizer. Absent means Curated: each
	 * harness's cost-effective catalog default. `"inherited"` summarizes with
	 * the thread's current model. Any other value names one explicit catalog
	 * model by its unique catalog id.
	 */
	compaction_model: Schema.optional(Schema.NonEmptyString),
	/**
	 * Engines the user has switched off entirely. A disabled engine's models
	 * are not represented as available anywhere — selectors, usage reads, and
	 * settings all treat it as absent until it is switched back on. Stored as
	 * the disabled set rather than the enabled set so a newly added harness
	 * arrives enabled instead of silently missing.
	 */
	disabled_engines: Schema.optional(Schema.Array(Identifier).check(Schema.isMaxLength(32))),
	/** The exact catalog model id most recently chosen, including its harness identity. */
	last_model_id: Schema.optional(Schema.NonEmptyString),
	models: Schema.Array(SessionModelDefaults).check(
		Schema.isMaxLength(session_defaults_maximum_models),
	),
	/** The harness-neutral permission option id, shared across all models. */
	permission: Identifier,
});

export type SessionDefaults = typeof SessionDefaults.Type;

/**
 * Patches session defaults. Every field is optional and absent means unchanged,
 * so a composer that only moved the effort slider does not restate the rest and
 * cannot race another client into overwriting it.
 *
 * @since 0.8.0
 */
export const SessionDefaultsUpdateInput = Schema.Struct({
	auto_continue_usage_limits: Schema.optional(Schema.Boolean),
	agent_name_dataset: Schema.optional(AgentNameDataset),
	/** `null` restores Curated: each harness's cost-effective catalog default. */
	compaction_model: Schema.optional(Schema.NullOr(Schema.NonEmptyString)),
	/** Switches one engine's availability without restating the others. */
	engine: Schema.optional(
		Schema.Struct({
			enabled: Schema.Boolean,
			engine_id: Identifier,
		}),
	),
	last_model_id: Schema.optional(Schema.NonEmptyString),
	model: Schema.optional(SessionModelDefaultsUpdate),
	permission: Schema.optional(Identifier),
});

export type SessionDefaultsUpdateInput = typeof SessionDefaultsUpdateInput.Type;

/** Commands one durable change to the session defaults. */
export const SessionDefaultsUpdateCommand = Schema.Struct({
	...SessionDefaultsUpdateInput.fields,
	type: Schema.Literal("session.defaults.update"),
});

export type SessionDefaultsUpdateCommand = typeof SessionDefaultsUpdateCommand.Type;

/** Announces the complete defaults after a durable change. */
export const SessionDefaultsUpdatedEvent = Schema.Struct({
	defaults: SessionDefaults,
	type: Schema.Literal("session.defaults.updated"),
});

export type SessionDefaultsUpdatedEvent = typeof SessionDefaultsUpdatedEvent.Type;
