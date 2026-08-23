import { Chunk } from "effect";
import type { Deferred, Semaphore } from "effect";

import type {
	EventEnvelope,
	MarketplaceScope,
	StreamCursor,
	SurfaceListQuery,
	SurfaceUsageAggregateQuery,
} from "@artisan/protocol";

/**
 * The retransmission window: events delivered to a connection but not yet
 * acknowledged, kept so a reconnect can replay exactly what was missed.
 *
 * A `Chunk` rather than an array because this is appended to on every single
 * event delivery and drained only by an ACK. Rebuilding an immutable array per
 * append is an O(n) copy each time, so a window of n events costs O(n²) bytes
 * to fill — at roughly 600 bytes an event that is gigabytes of pure copying for
 * a window of a few thousand, which is how Forge reached seventeen gigabytes of
 * transient allocation and died against V8's heap ceiling. `Chunk.appendAll`
 * shares structure instead, so filling the window costs what the events
 * themselves cost and nothing more.
 */
export type UnacknowledgedEvents = Chunk.Chunk<EventEnvelope>;

export interface PendingHeartbeat {
	readonly deadline_ms: number;
	readonly message_id: string;
	readonly nonce: string;
}

/** One connection-owned admission that has claimed an id but has not completed publication. */
export interface PendingSubscription {
	readonly cancelled: Deferred.Deferred<void>;
	/** Completes only after this generation's terminal envelope was accepted or the connection closed. */
	readonly released: Deferred.Deferred<void>;
	readonly message_id: string;
	/** Serializes publication and cancellation for only this subscription generation. */
	readonly publication: Semaphore.Semaphore;
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

export interface ThreadWorkProjectionSubscription {
	readonly _tag: "thread.work";
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
	| ThreadWorkProjectionSubscription
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
			readonly pending_subscriptions?: Readonly<Record<string, PendingSubscription>>;
			/** A cancelled generation retains its id until its terminal envelope is accepted. */
			readonly stopping_subscriptions?: Readonly<Record<string, PendingSubscription>>;
			/**
			 * Owns the active registration generation. It deliberately survives pending
			 * admission completion, so an old finalizer can never remove a reused id.
			 */
			readonly subscription_claims?: Readonly<Record<string, PendingSubscription>>;
			readonly stream_ticket: string;
			readonly subscriptions: Readonly<Record<string, ProjectionSubscription>>;
			/** Already-validated events delivered after the durable acknowledged point. */
			readonly unacknowledged_events?: UnacknowledgedEvents;
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

/** Reconstructs one ACK boundary from its durable base and delivered in-flight events. */
export const CursorsAtAcknowledgement = (
	acknowledged_cursors: Readonly<Record<string, number>>,
	unacknowledged_events: UnacknowledgedEvents,
	journal_sequence: number,
) =>
	ApplyEventCursors(
		acknowledged_cursors,
		Chunk.toArray(
			Chunk.filter(
				unacknowledged_events,
				(event) => event.journal_sequence <= journal_sequence,
			),
		),
	);

export const SameCursors = (
	left: Readonly<Record<string, number>>,
	right: Readonly<Record<string, number>>,
) => {
	const left_entries = Object.entries(left);
	const right_entries = Object.entries(right);

	return (
		left_entries.length === right_entries.length &&
		left_entries.every(([stream_id, sequence]) => right[stream_id] === sequence)
	);
};

/**
 * Applies only subscription entries derived from `baseline` that this fiber still owns.
 * A delayed projection must not restore an entry that a concurrent unsubscribe removed,
 * or overwrite a replacement registered under the same subscription id.
 */
export const MergeOwnedSubscriptionUpdates = (
	latest: ReadyState,
	baseline: Readonly<Record<string, ProjectionSubscription>>,
	updated: Readonly<Record<string, ProjectionSubscription>>,
) => {
	let subscriptions = latest.subscriptions;

	for (const [subscription_id, baseline_subscription] of Object.entries(baseline)) {
		const updated_subscription = updated[subscription_id];
		if (
			updated_subscription === undefined ||
			updated_subscription === baseline_subscription ||
			latest.subscriptions[subscription_id] !== baseline_subscription
		)
			continue;
		subscriptions = { ...subscriptions, [subscription_id]: updated_subscription };
	}

	return subscriptions;
};

/** Removes a subscription only when the caller still owns its registered generation. */
export const RemoveOwnedSubscription = (
	latest: ReadyState,
	baseline: Readonly<Record<string, ProjectionSubscription>>,
	subscription_id: string,
) => {
	if (
		baseline[subscription_id] === undefined ||
		latest.subscriptions[subscription_id] !== baseline[subscription_id]
	)
		return latest.subscriptions;
	const { [subscription_id]: _removed, ...subscriptions } = latest.subscriptions;
	return subscriptions;
};

export const LatestJournalSequence = (fallback: number, events: ReadonlyArray<EventEnvelope>) =>
	events.reduce((sequence, event) => Math.max(sequence, event.journal_sequence), fallback);
