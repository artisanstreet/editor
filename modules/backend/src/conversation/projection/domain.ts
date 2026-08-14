import { createHash } from "node:crypto";

import { conversation_body_text_limit } from "@artisan/protocol";
import type { ConversationErrorRef } from "@artisan/protocol";
import type { EngineErrorRef } from "@artisan/engines";

export class ConversationProjectionError extends Error {
	readonly _tag = "ConversationProjectionError";
}

export interface ConversationObservationContext {
	readonly agent_id?: string;
	readonly occurred_at: string;
	/** Parent renderer turn for observations authored by a delegated worker. */
	readonly parent_run_id?: string;
	readonly run_id: string;
	readonly thread_id: string;
}

export const lifecycle = (state: string) =>
	state === "completed" || state === "closed"
		? "completed"
		: state === "cancelled"
			? "cancelled"
			: state === "failed" || state === "interrupted"
				? "failed"
				: state === "waiting"
					? "waiting"
					: state === "pending"
						? "pending"
						: state === "streaming"
							? "streaming"
							: "active";

export const source_refs = (
	reference: string,
	input: { event_id?: string; journal_sequence?: number; provider?: string },
) => [
	{
		reference,
		...(input.event_id === undefined ? {} : { event_id: input.event_id }),
		...(input.journal_sequence === undefined
			? {}
			: { journal_sequence: input.journal_sequence }),
		...(input.provider === undefined ? {} : { provider: input.provider }),
	},
];

export const text = (value: string) => value.slice(0, 4_096);

/**
 * Bounds a message body rather than a label. Cutting a reply at label length
 * ends the turn mid-sentence with nothing saying it was cut, so bodies keep the
 * far wider protocol bound.
 */
export const body_text = (value: string) => value.slice(0, conversation_body_text_limit);

export const optional_text = (value: string | undefined) => {
	const normalized = value?.trim();

	return normalized ? text(normalized) : undefined;
};

const renderer_safe_terminal_details = new Set([
	"Artisan could not finish preparing the engine run.",
	"Engine startup was interrupted before the native session became ready.",
	"The engine did not become ready before the startup deadline.",
	"The engine failed before its native session became ready.",
	"The engine stopped before the run could finish.",
	"The engine could not deliver observations fast enough to continue safely.",
]);

/**
 * Projects only fixed Artisan terminal wording. Provider diagnostics and
 * arbitrary failure text remain private raw provenance rather than UI detail.
 */
export const terminal_failure = (error_ref: EngineErrorRef | undefined): ConversationErrorRef => ({
	code: error_ref?.artisan_code ?? "AE-RUN-301",
	...(error_ref?.detail !== undefined && renderer_safe_terminal_details.has(error_ref.detail)
		? { detail: error_ref.detail }
		: {}),
});

/** Keeps provider-native compaction identities out of renderer-visible item ids. */
export const compaction_item_id = (
	run_id: string,
	observation_id: string,
	compaction_id?: string,
) =>
	compaction_id === undefined
		? `compaction:${observation_id}`
		: `compaction:${createHash("sha256")
				.update(run_id)
				.update("\0")
				.update(compaction_id)
				.digest("hex")
				.slice(0, 24)}`;

export const turn_base = (
	id: string,
	input: ConversationObservationContext,
	state = "active",
	reference = id,
) => ({
	id,
	type: "turn" as const,
	created_at: input.occurred_at,
	updated_at: input.occurred_at,
	lifecycle: lifecycle(state),
	references: [],
	source_refs: source_refs(reference, { provider: "engine" }),
	...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
	...(input.parent_run_id === undefined ? {} : { parent_id: `run:${input.parent_run_id}` }),
	run_id: input.run_id,
});

export const item_base = (
	id: string,
	turn_id: string,
	input: ConversationObservationContext,
	state = "active",
	reference = id,
) => ({
	id,
	turn_id,
	created_at: input.occurred_at,
	updated_at: input.occurred_at,
	lifecycle: lifecycle(state),
	references: [],
	source_refs: source_refs(reference, { provider: "engine" }),
	...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
	run_id: input.run_id,
});
