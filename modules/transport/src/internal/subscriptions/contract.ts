import type { Effect, Option, Scope, Stream } from "effect";

import type {
	EventEnvelope,
	OutboundControlEnvelope,
	ProtocolErrorDetail,
	SurfaceListQuery,
	SurfaceUsageAggregateQuery,
} from "@artisan/protocol";

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
import type { SendCurrent } from "../client-common";
import type { ProjectionEnvelope } from "./model";

/** Owns applied durable cursors and connection-local projection subscriptions. */
export interface ClientSubscriptionCoordinator {
	readonly ApplyEvent: (
		event: EventEnvelope,
	) => Effect.Effect<ArtisanClientCursors, ArtisanClientError>;
	readonly ApplyReplayComplete: (
		envelope: Extract<OutboundControlEnvelope, { kind: "replay.complete" }>,
	) => Effect.Effect<void, ArtisanClientError>;
	readonly AwaitReady: Effect.Effect<void, ArtisanClientError>;
	readonly Cursors: Effect.Effect<ArtisanClientCursors>;
	readonly Dispose: (error: Option.Option<ArtisanClientError>) => Effect.Effect<void>;
	readonly Events: Stream.Stream<EventEnvelope, ArtisanClientError>;
	readonly HandleStarted: (
		envelope: Extract<OutboundControlEnvelope, { kind: "subscription.started" }>,
	) => Effect.Effect<void, ArtisanClientError>;
	readonly HandleUpdate: (envelope: ProjectionEnvelope) => Effect.Effect<void>;
	readonly Reject: (
		correlation_id: string,
		detail: ProtocolErrorDetail,
	) => Effect.Effect<boolean, ArtisanClientError>;
	readonly ResetConnection: Effect.Effect<void>;
	readonly ResumeCursors: Effect.Effect<ArtisanClientCursors>;
	readonly Retry: (send?: SendCurrent) => Effect.Effect<void, ArtisanClientError>;
	readonly SubscribeOrchestrationGraph: (
		group_id: string,
	) => Effect.Effect<
		Stream.Stream<OrchestrationGraphUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeThreadList: Effect.Effect<
		Stream.Stream<ThreadListUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeProjects: Effect.Effect<
		Stream.Stream<ProjectCatalogUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeThreadTranscript: (
		thread_id: string,
	) => Effect.Effect<
		Stream.Stream<ThreadTranscriptUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeConversation: (
		thread_id: string,
	) => Effect.Effect<
		Stream.Stream<ConversationUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeOrchestrationGroups: (
		thread_id: string,
		include_terminal: boolean,
	) => Effect.Effect<
		Stream.Stream<OrchestrationGroupListUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeThreadSession: (
		thread_id: string,
	) => Effect.Effect<
		Stream.Stream<ThreadSessionUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeSurfaceItems: (
		input: SurfaceListQuery,
	) => Effect.Effect<
		Stream.Stream<SurfaceListUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeSurfaceUsageAggregate: (
		input: SurfaceUsageAggregateQuery,
	) => Effect.Effect<
		Stream.Stream<SurfaceUsageAggregateUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
	readonly SubscribeWorkspaceConflicts: (
		thread_id: string,
	) => Effect.Effect<
		Stream.Stream<WorkspaceConflictListUpdate, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
}
