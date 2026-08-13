import { Identifier, JournalSequence, StreamCursor, StreamSequence } from "../common";

import {
	ConversationPatchBatch,
	ConversationSnapshot,
	ConversationSubscriptionCursor,
} from "../conversation";

import { OrchestrationGroupListSnapshot } from "../orchestration-groups";

import { ProjectCatalogSnapshot } from "../project";

import {
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
} from "../surfaces";

import { ThreadListItem } from "../thread";

import { ThreadTranscriptSnapshot, TranscriptEntry } from "../transcript";

import { WorkspaceConflictListQueryResult } from "../workspace-changes";

import { OrchestrationGraph, ThreadSessionSnapshot } from "./lifecycle";

import { Schema } from "effect";

import { NegotiatedBackendTraceMetadata, NegotiatedFrontendTraceMetadata } from "./trace";

/** Requests ordered updates for the thread-list projection. */
export const SubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("subscribe"),
	payload: Schema.Union([
		Schema.Struct({ type: Schema.Literal("project.list") }),
		Schema.Struct({ type: Schema.Literal("thread.list") }),
		Schema.Struct({ type: Schema.Literal("orchestration.graph"), group_id: Identifier }),
		Schema.Struct({ type: Schema.Literal("thread.transcript"), thread_id: Identifier }),
		Schema.Struct({
			type: Schema.Literal("conversation"),
			thread_id: Identifier,
			cursor: Schema.optional(ConversationSubscriptionCursor),
		}),
		Schema.Struct({
			type: Schema.Literal("orchestration.group.list"),
			thread_id: Identifier,
			include_terminal: Schema.Boolean,
		}),
		Schema.Struct({ type: Schema.Literal("thread.session"), thread_id: Identifier }),
		Schema.Struct({ type: Schema.Literal("surface.list"), query: SurfaceListQuery }),
		Schema.Struct({ type: Schema.Literal("workspace.conflict.list"), thread_id: Identifier }),
		Schema.Struct({
			type: Schema.Literal("surface.usage.aggregate"),
			query: SurfaceUsageAggregateQuery,
		}),
	]),
	subscription_id: Identifier,
});

export type SubscribeEnvelope = typeof SubscribeEnvelope.Type;

/** Stops delivery for a previously established projection subscription. */
export const UnsubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("unsubscribe"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type UnsubscribeEnvelope = typeof UnsubscribeEnvelope.Type;

/** Confirms that the backend has established an ordered projection subscription. */
export const SubscriptionStartedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.started"),
	payload: Schema.Struct({
		stream_id: Identifier,
	}),
	subscription_id: Identifier,
});

export type SubscriptionStartedEnvelope = typeof SubscriptionStartedEnvelope.Type;

/** Confirms that the backend has stopped an ordered projection subscription. */
export const SubscriptionStoppedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.stopped"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type SubscriptionStoppedEnvelope = typeof SubscriptionStoppedEnvelope.Type;

/** Provides ephemeral thread-list state; reconnecting clients request a fresh snapshot. */
export const ThreadListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.snapshot"),
	payload: Schema.Struct({
		threads: Schema.Array(ThreadListItem),
	}),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListSnapshotEnvelope = typeof ThreadListSnapshotEnvelope.Type;

/** Delivers one connection-local thread-list upsert after a subscription snapshot. */
export const ThreadListUpsertEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.upsert"),
	payload: ThreadListItem,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListUpsertEnvelope = typeof ThreadListUpsertEnvelope.Type;

/** Removes one erased thread from an ordered connection-local thread list. */
export const ThreadListRemoveEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.remove"),
	payload: Schema.Struct({ thread_id: Identifier }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListRemoveEnvelope = typeof ThreadListRemoveEnvelope.Type;

/** Provides the current Forge-owned project catalog for one ordered subscription. */
export const ProjectListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("project.list.snapshot"),
	payload: ProjectCatalogSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ProjectListSnapshotEnvelope = typeof ProjectListSnapshotEnvelope.Type;

/** Replaces the Forge-owned project catalog after an attach or detach mutation. */
export const ProjectListUpdatedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("project.list.updated"),
	payload: ProjectCatalogSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ProjectListUpdatedEnvelope = typeof ProjectListUpdatedEnvelope.Type;

/** Provides the initial graph projection for one ordered subscription. */
export const OrchestrationGraphSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.graph.snapshot"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type OrchestrationGraphSnapshotEnvelope = typeof OrchestrationGraphSnapshotEnvelope.Type;

/** Delivers an ordered replacement patch for one graph subscription. */
export const OrchestrationGraphPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.graph.patch"),
	payload: Schema.Struct({ graph: OrchestrationGraph }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type OrchestrationGraphPatchEnvelope = typeof OrchestrationGraphPatchEnvelope.Type;

export const ThreadTranscriptSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.transcript.snapshot"),
	payload: ThreadTranscriptSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadTranscriptSnapshotEnvelope = typeof ThreadTranscriptSnapshotEnvelope.Type;

export const ThreadTranscriptAppendEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.transcript.append"),
	payload: Schema.Struct({ entries: Schema.Array(TranscriptEntry) }),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadTranscriptAppendEnvelope = typeof ThreadTranscriptAppendEnvelope.Type;

export const ConversationSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("conversation.snapshot"),
	payload: ConversationSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ConversationSnapshotEnvelope = typeof ConversationSnapshotEnvelope.Type;

export const ConversationPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("conversation.patch"),
	payload: ConversationPatchBatch,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ConversationPatchEnvelope = typeof ConversationPatchEnvelope.Type;

export const OrchestrationGroupListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.group.list.snapshot"),
	payload: OrchestrationGroupListSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type OrchestrationGroupListSnapshotEnvelope =
	typeof OrchestrationGroupListSnapshotEnvelope.Type;

export const OrchestrationGroupListPatchEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("orchestration.group.list.patch"),
	payload: OrchestrationGroupListSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type OrchestrationGroupListPatchEnvelope = typeof OrchestrationGroupListPatchEnvelope.Type;

export const ThreadSessionSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.session.snapshot"),
	payload: ThreadSessionSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type ThreadSessionSnapshotEnvelope = typeof ThreadSessionSnapshotEnvelope.Type;
export const SurfaceListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("surface.list.snapshot"),
	payload: SurfaceSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type SurfaceListSnapshotEnvelope = typeof SurfaceListSnapshotEnvelope.Type;
export const SurfaceUsageAggregateSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("surface.usage.aggregate.snapshot"),
	payload: SurfaceUsageAggregateSnapshot,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type SurfaceUsageAggregateSnapshotEnvelope =
	typeof SurfaceUsageAggregateSnapshotEnvelope.Type;
export const WorkspaceConflictListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("workspace.conflict.list.snapshot"),
	payload: WorkspaceConflictListQueryResult,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});
export type WorkspaceConflictListSnapshotEnvelope =
	typeof WorkspaceConflictListSnapshotEnvelope.Type;

/** Acknowledges the highest contiguous journal position and durable event cursors. */
export const AckEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("ack"),
	payload: Schema.Struct({
		event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type AckEnvelope = typeof AckEnvelope.Type;

/** Requests durable replay after a global journal cursor with optional stream checks. */
export const ReplayEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("replay"),
	payload: Schema.Struct({
		after_journal_sequence: JournalSequence,
		event_cursors: Schema.optional(Schema.Array(StreamCursor)),
	}),
});

export type ReplayEnvelope = typeof ReplayEnvelope.Type;

/** Marks a replay boundary and reports the current global and stream positions. */
export const ReplayCompleteEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("replay.complete"),
	payload: Schema.Struct({
		current_event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type ReplayCompleteEnvelope = typeof ReplayCompleteEnvelope.Type;

/** Checks client liveness from the backend side of an idle control connection. */
export const HeartbeatPingEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("heartbeat.ping"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPingEnvelope = typeof HeartbeatPingEnvelope.Type;

/** Responds from the frontend using the backend ping identifier and nonce. */
export const HeartbeatPongEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("heartbeat.pong"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPongEnvelope = typeof HeartbeatPongEnvelope.Type;
