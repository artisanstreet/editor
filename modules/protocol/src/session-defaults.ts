import { Schema } from "effect";

import { Identifier } from "./common";

/**
 * Defaults a new thread inherits, owned by Forge rather than any one browser.
 *
 * Split by what each setting is about. `permission` is a statement about how
 * much the agent may do in your workspace, so it is shared across every model
 * and engine and stored once. Reasoning effort and context window describe a
 * particular model — not every model offers `max`, and context suffixes are
 * native to their harness — so they are stored per model and never coerced onto
 * a model that cannot express them.
 *
 * @since 0.8.0
 */

const session_defaults_maximum_models = 512;

/** Records the controls one model was last configured with. */
export const SessionModelDefaults = Schema.Struct({
	/** The native context-window suffix, absent for the model's base window. */
	context_window: Schema.optional(Schema.NonEmptyString),
	model_id: Schema.NonEmptyString,
	reasoning_effort: Schema.optional(Schema.Literals(["low", "medium", "high", "xhigh", "max"])),
});

export type SessionModelDefaults = typeof SessionModelDefaults.Type;

/** Projects every default a draft reads when it opens. */
export const SessionDefaults = Schema.Struct({
	/**
	 * The catalog model that generates handoff compaction summaries. Absent
	 * means each thread compacts with its own current model.
	 */
	compaction_model_id: Schema.optional(Schema.NonEmptyString),
	/** The model most recently chosen in any composer. */
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
	/** `null` clears the override so threads compact with their own model. */
	compaction_model_id: Schema.optional(Schema.NullOr(Schema.NonEmptyString)),
	last_model_id: Schema.optional(Schema.NonEmptyString),
	model: Schema.optional(SessionModelDefaults),
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
