import type { Cause, Deferred, Option, Queue } from "effect";

import type { EventEnvelope, OutboundControlEnvelope, SubscribeEnvelope } from "@artisan/protocol";

import type { SendCurrent } from "../client-common";
import type {
	ArtisanClientCursors,
	ArtisanClientError,
	ConversationUpdate,
	OrchestrationGraphUpdate,
	OrchestrationGroupListUpdate,
	ProjectCatalogUpdate,
	SurfaceListUpdate,
	SurfaceUsageAggregateUpdate,
	ThreadListUpdate,
	ThreadSessionUpdate,
	ThreadTranscriptUpdate,
	WorkspaceConflictListUpdate,
} from "../../client-api/service";

interface ProjectionSubscriptionBase {
	readonly envelope: SubscribeEnvelope;
	readonly expected_sequence: number;
	readonly started: Deferred.Deferred<void, ArtisanClientError>;
	readonly stream_id: Option.Option<string>;
}

interface ProjectionSubscriptionOf<Tag, Update> extends ProjectionSubscriptionBase {
	readonly _tag: Tag;
	readonly queue: Queue.Queue<Update, ArtisanClientError | Cause.Done<void>>;
}

export type ThreadListSubscription = ProjectionSubscriptionOf<"thread.list", ThreadListUpdate>;
export type ProjectCatalogSubscription = ProjectionSubscriptionOf<
	"project.list",
	ProjectCatalogUpdate
>;
export type OrchestrationGraphSubscription = ProjectionSubscriptionOf<
	"orchestration.graph",
	OrchestrationGraphUpdate
>;
export type ThreadTranscriptSubscription = ProjectionSubscriptionOf<
	"thread.transcript",
	ThreadTranscriptUpdate
>;
export type ConversationSubscription = ProjectionSubscriptionOf<"conversation", ConversationUpdate>;
export type OrchestrationGroupListSubscription = ProjectionSubscriptionOf<
	"orchestration.group.list",
	OrchestrationGroupListUpdate
>;
export type ThreadSessionSubscription = ProjectionSubscriptionOf<
	"thread.session",
	ThreadSessionUpdate
>;
export type SurfaceListSubscription = ProjectionSubscriptionOf<"surface.list", SurfaceListUpdate>;
export type SurfaceUsageAggregateSubscription = ProjectionSubscriptionOf<
	"surface.usage.aggregate",
	SurfaceUsageAggregateUpdate
>;
export type WorkspaceConflictListSubscription = ProjectionSubscriptionOf<
	"workspace.conflict.list",
	WorkspaceConflictListUpdate
>;

export type ProjectionSubscription =
	| OrchestrationGraphSubscription
	| ProjectCatalogSubscription
	| ThreadListSubscription
	| ThreadTranscriptSubscription
	| ConversationSubscription
	| OrchestrationGroupListSubscription
	| ThreadSessionSubscription
	| SurfaceListSubscription
	| SurfaceUsageAggregateSubscription
	| WorkspaceConflictListSubscription;

export type ProjectionEnvelope = Extract<
	OutboundControlEnvelope,
	{
		readonly kind:
			| "orchestration.graph.patch"
			| "orchestration.graph.snapshot"
			| "thread.list.snapshot"
			| "thread.list.upsert"
			| "thread.list.remove"
			| "project.list.snapshot"
			| "project.list.updated"
			| "thread.transcript.snapshot"
			| "thread.transcript.append"
			| "conversation.snapshot"
			| "conversation.patch"
			| "orchestration.group.list.snapshot"
			| "orchestration.group.list.patch"
			| "thread.session.snapshot"
			| "surface.list.snapshot"
			| "surface.usage.aggregate.snapshot"
			| "workspace.conflict.list.snapshot";
	}
>;

export interface SubscriptionState {
	readonly disposed: boolean;
	readonly event_observers: ReadonlyMap<
		string,
		Queue.Queue<EventEnvelope, ArtisanClientError | Cause.Done<void>>
	>;
	readonly event_cursors: Readonly<Record<string, number>>;
	readonly event_terminal: EventTerminal;
	readonly ignored_correlations: ReadonlySet<string>;
	readonly last_journal_sequence: number;
	/**
	 * The sender bound to the session currently replaying subscriptions, held
	 * from the replay until the session's state is reset. A subscription
	 * created inside that window must go out through it: the general current-
	 * connection path holds envelopes until the session is fully ready, and
	 * the replay's snapshot has already been taken — so without this sender
	 * the subscribe would never leave the client at all.
	 */
	readonly session_send: Option.Option<SendCurrent>;
	readonly subscriptions: ReadonlyMap<string, ProjectionSubscription>;
}

export type EventTerminal =
	| { readonly _tag: "active" }
	| { readonly _tag: "ended" }
	| { readonly _tag: "failed"; readonly error: ArtisanClientError };

export type EventApplication =
	| { readonly _tag: "Applied"; readonly cursors: ArtisanClientCursors }
	| { readonly _tag: "Duplicate" }
	| { readonly _tag: "Gap" }
	| {
			readonly _tag: "Overflow";
			readonly observers: ReadonlyArray<
				Queue.Queue<EventEnvelope, ArtisanClientError | Cause.Done<void>>
			>;
	  };

export type SubscriptionDelivery =
	| { readonly _tag: "Delivered" }
	| { readonly _tag: "Ignored" }
	| {
			readonly _tag: "Gap" | "Overflow";
			readonly subscription: ProjectionSubscription;
	  };

export type SubscriptionStart =
	| { readonly _tag: "Duplicate" }
	| { readonly _tag: "Found"; readonly subscription: ProjectionSubscription }
	| { readonly _tag: "Ignored" }
	| { readonly _tag: "Missing" };

export type SubscriptionRejection = Exclude<SubscriptionStart, { readonly _tag: "Duplicate" }>;
export type ProjectionOffer = "mismatch" | "offered" | "overflow";
