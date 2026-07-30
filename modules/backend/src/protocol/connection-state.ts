import type {
	EventEnvelope,
	MarketplaceScope,
	StreamCursor,
	SurfaceListQuery,
	SurfaceUsageAggregateQuery,
} from "@artisan/protocol";

export interface PendingHeartbeat {
	readonly deadline_ms: number;
	readonly message_id: string;
	readonly nonce: string;
}

export interface ThreadListProjectionSubscription {
	readonly _tag: "thread.list";
	readonly sequence: number;
	readonly stream_id: string;
}

export interface ProjectListProjectionSubscription {
	readonly _tag: "project.list";
	readonly sequence: number;
	readonly stream_id: string;
}

export interface OrchestrationGraphProjectionSubscription {
	readonly _tag: "orchestration.graph";
	readonly group_id: string;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface ThreadTranscriptProjectionSubscription {
	readonly _tag: "thread.transcript";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface ConversationProjectionSubscription {
	readonly _tag: "conversation";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly patch_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface OrchestrationGroupListProjectionSubscription {
	readonly _tag: "orchestration.group.list";
	readonly thread_id: string;
	readonly include_terminal: boolean;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface ThreadSessionProjectionSubscription {
	readonly _tag: "thread.session";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface WorkspaceConflictListProjectionSubscription {
	readonly _tag: "workspace.conflict.list";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface SurfaceListProjectionSubscription {
	readonly _tag: "surface.list";
	readonly query: SurfaceListQuery;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export interface SurfaceUsageProjectionSubscription {
	readonly _tag: "surface.usage.aggregate";
	readonly thread_id?: string;
	readonly query: SurfaceUsageAggregateQuery;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

export type ProjectionSubscription =
	| ThreadListProjectionSubscription
	| ProjectListProjectionSubscription
	| OrchestrationGraphProjectionSubscription
	| ThreadTranscriptProjectionSubscription
	| ConversationProjectionSubscription
	| OrchestrationGroupListProjectionSubscription
	| ThreadSessionProjectionSubscription
	| WorkspaceConflictListProjectionSubscription
	| SurfaceListProjectionSubscription
	| SurfaceUsageProjectionSubscription;

export type ConnectionState =
	| {
			readonly _tag: "AwaitingHello";
			readonly last_activity_ms: number;
	  }
	| {
			readonly _tag: "Ready";
			readonly acknowledged_cursors: Readonly<Record<string, number>>;
			readonly acknowledged_journal_sequence: number;
			readonly connection_id: string;
			readonly delivered_cursors: Readonly<Record<string, number>>;
			readonly delivered_journal_sequence: number;
			readonly last_activity_ms: number;
			readonly pending_heartbeat?: PendingHeartbeat;
			readonly stream_ticket: string;
			readonly subscriptions: Readonly<Record<string, ProjectionSubscription>>;
	  }
	| {
			readonly _tag: "Rejected";
			readonly last_activity_ms: number;
	  }
	| { readonly _tag: "Closed" };

export type AwaitingHelloState = Extract<ConnectionState, { readonly _tag: "AwaitingHello" }>;
export type ReadyState = Extract<ConnectionState, { readonly _tag: "Ready" }>;

export const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(left.kind === "workspace" &&
			right.kind === "workspace" &&
			left.workspace_id === right.workspace_id) ||
		(left.kind === "project" &&
			right.kind === "project" &&
			left.project_id === right.project_id));

export const CursorsToRecord = (cursors: ReadonlyArray<StreamCursor>) =>
	Object.fromEntries(cursors.map((cursor) => [cursor.stream_id, cursor.sequence]));

export const RecordToCursors = (cursors: Readonly<Record<string, number>>) =>
	Object.entries(cursors)
		.map(([stream_id, sequence]) => ({ sequence, stream_id }))
		.sort((left, right) => left.stream_id.localeCompare(right.stream_id));

export const ApplyEventCursors = (
	cursors: Readonly<Record<string, number>>,
	events: ReadonlyArray<EventEnvelope>,
) =>
	events.reduce<Readonly<Record<string, number>>>(
		(current, event) => ({
			...current,
			[event.stream_id]: Math.max(current[event.stream_id] ?? 0, event.sequence),
		}),
		cursors,
	);

export const LatestJournalSequence = (fallback: number, events: ReadonlyArray<EventEnvelope>) =>
	events.reduce((sequence, event) => Math.max(sequence, event.journal_sequence), fallback);
