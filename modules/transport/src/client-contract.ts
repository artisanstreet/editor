import { Context, Data, Effect, Option, Scope, Stream } from "effect";

import type {
	CommandPayload,
	EventEnvelope,
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProvider,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
	GitDiffQuery,
	GitDiffQueryResult,
	GitMutationKind,
	GitMutationPaths,
	GitMutationResolveRequest,
	GitSnapshotId,
	GitWorkspaceQuery,
	GitWorkspaceQueryResult,
	ContentIdentity,
	ModelBehaviourDriftResolutionRequest,
	ModelBehaviourRetryRequest,
	ModelBehaviourSnapshot,
	ModelBehaviourUpdateRequest,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
	StreamCursor,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	RawOrigin,
	ThreadTranscriptQuery,
	ThreadTranscriptSnapshot,
	ThreadSessionSnapshot,
	TranscriptEntry,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
	WorkspaceFileReplaceRequest,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	WorkspaceChangeReview,
	WorkspaceConflictListQueryResult,
} from "@artisan/protocol";

/** Identifies a typed frontend client failure. */
export type ArtisanClientErrorCode =
	| "configuration"
	| "connection"
	| "correlation_conflict"
	| "disposed"
	| "event_overflow"
	| "malformed"
	| "protocol"
	| "request_overflow"
	| "stream_closed"
	| "stream_gap"
	| "stream_overflow"
	| "subscription_overflow";

/** Reports a transport, protocol, request, subscription, or stream client failure. */
export class ArtisanClientError extends Data.TaggedError("ArtisanClientError")<{
	readonly cause: unknown;
	readonly code: ArtisanClientErrorCode;
	readonly message: string;
	readonly protocol_code: string;
	readonly retryable: boolean;
}> {}

/** Supplies command intent while the client owns envelope ids and trace metadata. */
export interface ArtisanCommandInput {
	readonly agent_id?: string;
	readonly causation_id?: string;
	readonly command_id?: string;
	readonly payload: CommandPayload;
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Records the durable command outcome returned after acceptance or deduplication. */
export interface ArtisanCommandReceipt {
	readonly command_id: string;
	readonly journal_sequence: number;
	readonly status: "accepted" | "duplicate";
}

/** Supplies the workspace file identity needed for one read query. */
export interface ArtisanWorkspaceFileReadInput extends WorkspaceFileReadQuery {}

/** Supplies attributed replacement content and its optimistic-concurrency identity. */
export interface ArtisanWorkspaceFileReplaceInput extends WorkspaceFileReplaceRequest {
	readonly agent_id: string;
	readonly command_id?: string;
	readonly raw_origin?: RawOrigin;
	readonly run_id: string;
	readonly thread_id: string;
}

/** Supplies the thread and optional workspace scope for a change projection query. */
export interface ArtisanWorkspaceChangeListInput {
	readonly thread_id: string;
	readonly workspace_id?: string;
}

/** Supplies the thread and change identity for one workspace diff query. */
export interface ArtisanWorkspaceChangeDiffInput extends WorkspaceChangeDiffQuery {}

/** Supplies review metadata shared by user and graph review transitions. */
interface ArtisanWorkspaceChangeReviewInputBase {
	readonly change_id: string;
	readonly comment?: WorkspaceChangeReview["comment"];
	readonly command_id?: string;
	readonly outcome?: WorkspaceChangeReview["outcome"];
	readonly raw_origin?: RawOrigin;
	readonly thread_id: string;
}

/** Supplies an explicit user review without graph authority attribution. */
export interface ArtisanUserWorkspaceChangeReviewInput extends ArtisanWorkspaceChangeReviewInputBase {
	readonly assignment_id?: never;
	readonly group_id?: never;
	readonly reviewer_agent_id?: never;
	readonly reviewer_kind: "user";
	readonly reviewer_run_id?: never;
}

/** Supplies a graph review bound to one active reviewer assignment and run. */
export interface ArtisanGraphWorkspaceChangeReviewInput extends ArtisanWorkspaceChangeReviewInputBase {
	readonly assignment_id: string;
	readonly group_id: string;
	readonly reviewer_agent_id: string;
	readonly reviewer_kind: "graph";
	readonly reviewer_run_id: string;
}

/** Supplies the durable identity, review metadata, and reviewer authority. */
export type ArtisanWorkspaceChangeReviewInput =
	| ArtisanUserWorkspaceChangeReviewInput
	| ArtisanGraphWorkspaceChangeReviewInput;

/** Supplies the durable identity and attribution for a guarded rollback. */
export interface ArtisanWorkspaceChangeRollbackInput {
	readonly change_id: string;
	readonly command_id?: string;
	readonly expected_after: ContentIdentity;
	readonly thread_id: string;
}

/** Supplies the thread and workspace identity for one durable Git projection query. */
export interface ArtisanGitWorkspaceInput extends GitWorkspaceQuery {}

/** Supplies the exact snapshot and scope for one bounded Git diff query. */
export interface ArtisanGitDiffInput extends GitDiffQuery {}

/** Supplies one approval-bearing stage or unstage request with trace attribution. */
export interface ArtisanGitIndexMutationInput {
	readonly agent_id?: string;
	readonly approval_id?: string;
	readonly command_id?: string;
	readonly expected_snapshot_id: GitSnapshotId;
	readonly expected_workspace_version: number;
	readonly kind: GitMutationKind;
	readonly mutation_id?: string;
	readonly paths: GitMutationPaths;
	readonly raw_origin?: RawOrigin;
	readonly run_id?: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Resolves the approval bound to one exact Git mutation with trace attribution. */
export interface ArtisanGitMutationResolveInput extends GitMutationResolveRequest {
	readonly agent_id?: string;
	readonly command_id?: string;
	readonly raw_origin?: RawOrigin;
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Supplies the public retention setting and optional durable retry identity. */
export interface ArtisanThreadRetentionUpdateInput {
	readonly command_id?: string;
	readonly enabled: boolean;
	readonly inactivity_days: number;
}

/** Supplies canonical guidance content and an optional durable retry identity. */
export interface ArtisanGlobalGuidanceUpdateInput {
	readonly command_id?: string;
	readonly content: string;
}

/** Selects one first-run provider value while hiding internal settings scope ids. */
export interface ArtisanGlobalGuidanceSelectionInput extends GlobalGuidanceSelectionRequest {
	readonly command_id?: string;
}

/** Resolves one exact drift observation through an explicit recovery action. */
export interface ArtisanGlobalGuidanceDriftInput extends GlobalGuidanceDriftResolutionRequest {
	readonly command_id?: string;
}

/** Retries one provider's fixed sync policy without exposing a provider toggle. */
export interface ArtisanGlobalGuidanceRetryInput {
	readonly command_id?: string;
	readonly provider: GlobalGuidanceProvider;
}

/** Supplies one canonical Model Behaviour update and optional durable retry identity. */
export interface ArtisanModelBehaviourUpdateInput extends ModelBehaviourUpdateRequest {
	readonly command_id?: string;
}

/** Resolves one exact provider drift observation without exposing native config content. */
export interface ArtisanModelBehaviourDriftInput extends ModelBehaviourDriftResolutionRequest {
	readonly command_id?: string;
}

/** Retries one fixed provider mapping through the canonical control. */
export interface ArtisanModelBehaviourRetryInput extends ModelBehaviourRetryRequest {
	readonly command_id?: string;
}

/** Exposes the last event positions applied before an ACK or reconnect hello. */
export interface ArtisanClientCursors {
	readonly event_cursors: ReadonlyArray<StreamCursor>;
	readonly last_journal_sequence: number;
}

/** Delivers a complete graph snapshot or one ordered replacement patch. */
export type OrchestrationGraphUpdate =
	| {
			readonly graph: OrchestrationGraph;
			readonly journal_sequence: number;
			readonly type: "snapshot";
	  }
	| {
			readonly graph: OrchestrationGraph;
			readonly journal_sequence: number;
			readonly type: "patch";
	  };

/** Delivers a race-safe thread-list snapshot, upsert, or removal. */
export type ThreadListUpdate =
	| {
			readonly journal_sequence: number;
			readonly threads: ReadonlyArray<ThreadListItem>;
			readonly type: "snapshot";
	  }
	| {
			readonly journal_sequence: number;
			readonly thread: ThreadListItem;
			readonly type: "upsert";
	  }
	| {
			readonly journal_sequence: number;
			readonly thread_id: string;
			readonly type: "remove";
	  };

export type ThreadTranscriptUpdate =
	| {
			readonly type: "snapshot";
			readonly journal_sequence: number;
			readonly transcript: ThreadTranscriptSnapshot;
	  }
	| {
			readonly type: "append";
			readonly journal_sequence: number;
			readonly entries: ReadonlyArray<TranscriptEntry>;
	  };
export type OrchestrationGroupListUpdate =
	| { readonly type: "snapshot"; readonly snapshot: OrchestrationGroupListSnapshot }
	| { readonly type: "patch"; readonly snapshot: OrchestrationGroupListSnapshot };
export type ThreadSessionUpdate = {
	readonly type: "snapshot";
	readonly snapshot: ThreadSessionSnapshot;
};
export type SurfaceListUpdate = { readonly type: "snapshot"; readonly snapshot: SurfaceSnapshot };
export type SurfaceUsageAggregateUpdate = {
	readonly type: "snapshot";
	readonly snapshot: SurfaceUsageAggregateSnapshot;
};
export type WorkspaceConflictListUpdate = {
	readonly type: "snapshot";
	readonly snapshot: WorkspaceConflictListQueryResult;
};

/** Configures bounded client queues, reconnect timing, and request concurrency. */
export interface ArtisanClientOptions {
	readonly error_capacity?: number;
	readonly event_capacity?: number;
	readonly max_pending_requests?: number;
	readonly reconnect_delay_ms?: number;
	readonly stream_capacity?: number;
	readonly subscription_capacity?: number;
}

/**
 * Provides typed frontend operations while hiding protocol envelopes and cursors.
 * MessagePorts are reliable while alive: commands retry only after reconnect,
 * using the exact original envelope and id until a durable receipt arrives.
 */
export class ArtisanClient extends Context.Service<
	ArtisanClient,
	{
		readonly Command: (
			input: ArtisanCommandInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly Cursors: Effect.Effect<ArtisanClientCursors>;
		readonly Dispose: Effect.Effect<void>;
		readonly Errors: Stream.Stream<ArtisanClientError>;
		readonly Events: Stream.Stream<EventEnvelope, ArtisanClientError>;
		readonly GetOrchestrationGraph: (
			group_id: string,
		) => Effect.Effect<OrchestrationGraph, ArtisanClientError>;
		readonly GetThreadTranscript: (
			input: ThreadTranscriptQuery,
		) => Effect.Effect<ThreadTranscriptSnapshot, ArtisanClientError>;
		readonly GetThreadSession: (
			thread_id: string,
		) => Effect.Effect<ThreadSessionSnapshot, ArtisanClientError>;
		readonly ListSurfaceItems: (
			input: SurfaceListQuery,
		) => Effect.Effect<SurfaceSnapshot, ArtisanClientError>;
		readonly GetSurfaceUsageAggregate: (
			input: SurfaceUsageAggregateQuery,
		) => Effect.Effect<SurfaceUsageAggregateSnapshot, ArtisanClientError>;
		readonly ListOrchestrationGroups: (
			thread_id: string,
			include_terminal: boolean,
		) => Effect.Effect<OrchestrationGroupListSnapshot, ArtisanClientError>;
		readonly GetGlobalGuidance: Effect.Effect<GlobalGuidanceSnapshot, ArtisanClientError>;
		readonly GetGitDiff: (
			input: ArtisanGitDiffInput,
		) => Effect.Effect<GitDiffQueryResult, ArtisanClientError>;
		readonly GetGitWorkspace: (
			input: ArtisanGitWorkspaceInput,
		) => Effect.Effect<GitWorkspaceQueryResult, ArtisanClientError>;
		readonly GetModelBehaviour: Effect.Effect<ModelBehaviourSnapshot, ArtisanClientError>;
		readonly GetThreadRetentionPolicy: Effect.Effect<ThreadRetentionPolicy, ArtisanClientError>;
		readonly GetThreadWork: (
			thread_id: string,
		) => Effect.Effect<Option.Option<ThreadWorkItem>, ArtisanClientError>;
		readonly ListTerminals: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, ArtisanClientError>;
		readonly ListThreads: Effect.Effect<ReadonlyArray<ThreadListItem>, ArtisanClientError>;
		readonly ListWorkspaceChanges: (
			input: ArtisanWorkspaceChangeListInput,
		) => Effect.Effect<WorkspaceChangeListQueryResult, ArtisanClientError>;
		readonly ListWorkspaceConflicts: (
			thread_id: string,
		) => Effect.Effect<WorkspaceConflictListQueryResult, ArtisanClientError>;
		readonly GetWorkspaceChangeDiff: (
			input: ArtisanWorkspaceChangeDiffInput,
		) => Effect.Effect<WorkspaceChangeDiffQueryResult, ArtisanClientError>;
		readonly OpenAsset: (
			asset_id: string,
		) => Effect.Effect<
			Stream.Stream<Uint8Array, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly OpenTerminalOutput: (
			terminal_id: string,
		) => Effect.Effect<
			Stream.Stream<Uint8Array, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly ReadWorkspaceFile: (
			input: ArtisanWorkspaceFileReadInput,
		) => Effect.Effect<WorkspaceFileReadQueryResult, ArtisanClientError>;
		readonly SubscribeThreadList: Effect.Effect<
			Stream.Stream<ThreadListUpdate, ArtisanClientError>,
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
		readonly SubscribeOrchestrationGraph: (
			group_id: string,
		) => Effect.Effect<
			Stream.Stream<OrchestrationGraphUpdate, ArtisanClientError>,
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
		readonly ResolveGlobalGuidanceDrift: (
			input: ArtisanGlobalGuidanceDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RequestGitIndexMutation: (
			input: ArtisanGitIndexMutationInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveGitMutation: (
			input: ArtisanGitMutationResolveInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveModelBehaviourDrift: (
			input: ArtisanModelBehaviourDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RetryGlobalGuidanceSync: (
			input: ArtisanGlobalGuidanceRetryInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RetryModelBehaviourSync: (
			input: ArtisanModelBehaviourRetryInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly SelectGlobalGuidance: (
			input: ArtisanGlobalGuidanceSelectionInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UpdateGlobalGuidance: (
			input: ArtisanGlobalGuidanceUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UpdateModelBehaviour: (
			input: ArtisanModelBehaviourUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UpdateThreadRetentionPolicy: (
			input: ArtisanThreadRetentionUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ReplaceWorkspaceFile: (
			input: ArtisanWorkspaceFileReplaceInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ReviewWorkspaceChange: (
			input: ArtisanWorkspaceChangeReviewInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RollbackWorkspaceChange: (
			input: ArtisanWorkspaceChangeRollbackInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
	}
>()("Artisan/ArtisanClient") {}
