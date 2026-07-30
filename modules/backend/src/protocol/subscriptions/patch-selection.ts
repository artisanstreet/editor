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
