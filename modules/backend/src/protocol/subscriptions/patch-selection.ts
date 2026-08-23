import type { EventEnvelope, ThreadListItem } from "@artisan/protocol";

export type ThreadListProjectionPatch =
	| { readonly _tag: "Remove"; readonly thread_id: string }
	| { readonly _tag: "Upsert"; readonly thread: ThreadListItem };

const thread_item_from_event = (event: EventEnvelope): ThreadListItem | undefined => {
	if (event.payload.type === "thread.metadata.updated") return event.payload.thread;
	if (event.payload.type === "thread.project_affinity.updated") return event.payload.thread;
	return event.payload.type === "thread.created"
		? {
				activity_version: 0,
				affinity_version: 0,
				created_at: event.sent_at,
				current_goal: event.payload.title,
				last_activity_at: event.sent_at,
				last_message_at: event.sent_at,
				reader_activity_at: event.sent_at,
				live_status: "Idle",
				metadata_version: 0,
				pinned: false,
				linked_projects: [],
				project_affinity_scores: [],
				project_locked: false,
				thread_id: event.thread_id,
				title: event.payload.title,
				title_locked: false,
				title_source: "initial",
				updated_at: event.sent_at,
			}
		: undefined;
};

export const DirectThreadListPatch = (
	event: EventEnvelope,
): ThreadListProjectionPatch | undefined => {
	if (event.payload.type === "thread.erased")
		return { _tag: "Remove", thread_id: event.thread_id };
	const thread = thread_item_from_event(event);
	return thread ? { _tag: "Upsert", thread } : undefined;
};

export const GraphGroupId = (event: EventEnvelope) =>
	event.payload.type === "orchestration.graph.lifecycle" ||
	event.payload.type === "assignment.heartbeat" ||
	event.payload.type === "agent_instance.renamed" ||
	event.payload.type === "assignment.control" ||
	event.payload.type === "artifact.recorded"
		? event.payload.group_id
		: undefined;

/** Must match the durable allowlist in TranscriptReadModel plus terminal erasure. */
const transcript_event_types = new Set([
	"thread.message_queued",
	"thread.message_steering",
	"assistant.message_completed",
	"interaction.approval",
	"interaction.question",
	"intake.assessed",
	"intake.assumption_recorded",
	"thread.erased",
]);

/** These are the notifier-visible event forms emitted beside PersistSurfaceProjection. */
const surface_event_types = new Set([
	"assistant.message_completed",
	"interaction.approval",
	"interaction.question",
	"run.lifecycle",
	"usage.interruption.updated",
	"thread.erased",
]);

/** Workspace reconciliation writes only conflict updates; erasure clears the projection. */
const workspace_conflict_event_types = new Set(["workspace.conflict.updated", "thread.erased"]);

/**
 * `RunLifecycle.record_observation` writes the conversation and surface
 * projections before it publishes these provider-attributed graph forms. They
 * are deliberately narrower than every graph event: command-side graph
 * changes do not mutate either projection and must remain a zero-read wake.
 */
const provider_projection_graph_actions = new Set([
	"attempt_finished",
	"finished",
	"provider_state",
	"summary_recorded",
]);

const IsProviderProjectionEvent = (event: EventEnvelope) => {
	if (event.raw_origin === undefined) return false;
	if (event.payload.type === "artifact.recorded") return true;
	return (
		event.payload.type === "orchestration.graph.lifecycle" &&
		provider_projection_graph_actions.has(event.payload.action)
	);
};

/**
 * There is intentionally no `EventAffectsConversation`. Conversation patches
 * are written by projection transactions outside the journal, so no event
 * predicate can prove a wake carried none — conversation delivery drains
 * every subscription's cursor on every wake instead.
 */
export const EventAffectsTranscript = (event: EventEnvelope) =>
	transcript_event_types.has(event.payload.type);

export const EventAffectsSurface = (event: EventEnvelope) =>
	surface_event_types.has(event.payload.type) || IsProviderProjectionEvent(event);

export const EventAffectsWorkspaceConflicts = (event: EventEnvelope) =>
	workspace_conflict_event_types.has(event.payload.type);
