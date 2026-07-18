import { Schema } from "effect";
import { Identifier, IsoDateTime, JournalSequence, PositiveInt } from "./common";

/** Compact, thread-scoped orchestration discovery projection. */
export const OrchestrationGroupListQuery = Schema.Struct({
	include_terminal: Schema.Boolean,
	thread_id: Identifier,
});
export type OrchestrationGroupListQuery = typeof OrchestrationGroupListQuery.Type;

export const OrchestrationGroupSummary = Schema.Struct({
	coordinator_agent_id: Identifier,
	created_at: IsoDateTime,
	group_id: Identifier,
	max_concurrency: PositiveInt,
	state: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"blocked",
		"joining",
		"summarized",
		"stopped",
		"failed",
		"complete",
	]),
	thread_id: Identifier,
	updated_at: IsoDateTime,
	version: PositiveInt,
});
export type OrchestrationGroupSummary = typeof OrchestrationGroupSummary.Type;

export const OrchestrationGroupListSnapshot = Schema.Struct({
	groups: Schema.Array(OrchestrationGroupSummary),
	journal_sequence: JournalSequence,
});
export type OrchestrationGroupListSnapshot = typeof OrchestrationGroupListSnapshot.Type;
