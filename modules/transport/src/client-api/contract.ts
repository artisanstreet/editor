import { Data } from "effect";

import type {
	CommandPayload,
	CapabilityConnectPreviewRequest,
	CapabilityDetail,
	CapabilityDriftResolutionRequest,
	CapabilityDriftOverwriteDecision,
	CapabilityDriftOverwriteRequest,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationRequest,
	CapabilityOAuthCompleteRequest,
	CapabilityOAuthRequest,
	CapabilityHealthRequest,
	ConversationPatchBatch,
	ConversationSnapshot,
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProvider,
	GlobalGuidanceSelectionRequest,
	GitDiffQuery,
	GitMutationKind,
	GitMutationPaths,
	GitMutationResolveRequest,
	GitSnapshotId,
	GitWorkspaceQuery,
	ContentIdentity,
	ModelBehaviourDriftResolutionRequest,
	ModelBehaviourRetryRequest,
	ModelBehaviourUpdateRequest,
	MarketplaceApprovalDecision,
	MarketplaceBrowseQuery,
	MarketplaceSyncRequest,
	NpxSkillsDiscoveryRequest,
	NpxSkillsImportRequest,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	PreviewAssetMetadataQuery,
	PreviewInspectionRequest,
	PreviewInspectionSessionOpenRequest,
	PreviewTargetGetQuery,
	PreviewTargetRegistration,
	PreviewTargetStateRequest,
	RichLinkResolveQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateSnapshot,
	StreamCursor,
	ThreadListItem,
	RawOrigin,
	ThreadTranscriptSnapshot,
	ThreadSessionSnapshot,
	ThreadSessionPolicy,
	TranscriptEntry,
	RoutineDetail,
	RoutineDriftResolutionRequest,
	RoutineDriftOverwriteDecision,
	RoutineDriftOverwriteRequest,
	RoutineInstallRequest,
	RoutineInvocationRequest,
	RoutineRollbackRequest,
	WorkspaceFileReadQuery,
	WorkspaceFileReplaceRequest,
	WorkspaceChangeDiffQuery,
	ArtisanApprovalListQuery,
	ArtisanApprovalResolveRequest,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocationListQuery,
	ArtisanToolRegistryListQuery,
	WorkspaceFileDiscoveryQuery,
	WorkspaceLanguageCapabilitiesQuery,
	WorkspaceChangeReview,
	WorkspaceConflictListQueryResult,
} from "@artisan/protocol";
import type { ProjectCatalogSnapshot } from "@artisan/protocol";

/**
 * Identifies a typed frontend client failure. The V1 `event_overflow` and
 * `stream_overflow` names remain for public/peer compatibility; neither is
 * emitted by a bounded local queue.
 */
export type ArtisanClientErrorCode =
	| "configuration"
	| "connection"
	| "correlation_conflict"
	| "disposed"
	| "event_overflow"
	| "malformed"
	| "protocol"
	| "stream_closed"
	| "stream_gap"
	| "stream_not_found"
	| "stream_overflow";

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
	readonly payload: Exclude<CommandPayload, { readonly type: "thread.create" }>;
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

/** Supplies one local preview target registration without giving the renderer a browser capability. */
export interface ArtisanPreviewTargetRegistrationInput extends PreviewTargetRegistration {}

/** Supplies one explicit target id for a direct target action. */
export interface ArtisanPreviewTargetInput extends PreviewTargetGetQuery {}

/** Supplies one explicit target lifecycle state. */
export interface ArtisanPreviewTargetStateInput extends PreviewTargetStateRequest {}

/** Resolves safe, bounded rich-link metadata through the backend policy boundary. */
export interface ArtisanRichLinkResolveInput extends RichLinkResolveQuery {}

/** Opens an attributable external-browser inspection session. */
export interface ArtisanPreviewInspectionOpenInput extends PreviewInspectionSessionOpenRequest {}

/** Runs one bounded connector command within an explicit inspection session. */
export interface ArtisanPreviewInspectionInput extends PreviewInspectionRequest {}

/** Looks up retained asset metadata before opening its existing binary stream channel. */
export interface ArtisanPreviewAssetMetadataInput extends PreviewAssetMetadataQuery {}

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

/** Supplies policy and optional workspace context for the built-in tool registry. */
export interface ArtisanToolRegistryListInput extends ArtisanToolRegistryListQuery {}

/** Supplies invocation data plus durable trace attribution for one tool execution. */
export interface ArtisanToolExecuteInput extends ArtisanToolExecutionRequest {
	readonly agent_id?: string;
	readonly command_id?: string;
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Resolves one pending built-in tool approval with durable trace attribution. */
export interface ArtisanApprovalResolveInput extends ArtisanApprovalResolveRequest {
	readonly agent_id?: string;
	readonly command_id?: string;
	readonly raw_origin?: RawOrigin;
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Supplies cursor, filters, and thread scope for a renderer invocation timeline. */
export interface ArtisanToolInvocationListInput extends ArtisanToolInvocationListQuery {}

/** Supplies the thread and optional state filter for a renderer approval list. */
export interface ArtisanApprovalListInput extends ArtisanApprovalListQuery {}

/** Supplies the root-confined page cursor and optional prefix for workspace discovery. */
export interface ArtisanWorkspaceFileDiscoveryInput extends WorkspaceFileDiscoveryQuery {}

/** Supplies the workspace whose truthful language capability state is requested. */
export interface ArtisanWorkspaceLanguageCapabilitiesInput extends WorkspaceLanguageCapabilitiesQuery {}

/** Supplies the public retention setting and optional durable retry identity. */
export interface ArtisanThreadRetentionUpdateInput {
	readonly command_id?: string;
	readonly enabled: boolean;
	readonly inactivity_days: number;
}

/** Names the model to star or unstar, with an optional durable retry identity. */
export interface ArtisanModelFavoriteUpdateInput {
	readonly command_id?: string;
	readonly favorite: boolean;
	readonly model_id: string;
}

/** Supplies a durable thread-local Engine launch policy and optional retry identity. */
export interface ArtisanThreadSessionPolicyUpdateInput {
	readonly command_id?: string;
	readonly policy: ThreadSessionPolicy;
	readonly thread_id: string;
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

/** Supplies one progressive Marketplace category browse or search filter. */
export interface ArtisanMarketplaceBrowseInput extends MarketplaceBrowseQuery {}

/** Selects one scope-bound routine without leaking registry internals. */
export interface ArtisanRoutineDetailInput {
	readonly routine_id: string;
	readonly scope: RoutineDetail["scope"];
}

/** Binds a routine installation request to its inspected source and scope. */
export interface ArtisanRoutineInstallInput extends RoutineInstallRequest {
	readonly command_id?: string;
}

/** Resolves the exact inspected routine installation approval. */
export interface ArtisanRoutineApprovalInput extends MarketplaceApprovalDecision {
	readonly command_id?: string;
}

/** Carries one routine lifecycle id and an optional durable retry id. */
export interface ArtisanRoutineIdInput {
	readonly command_id?: string;
	readonly routine_id: string;
	readonly scope: RoutineDetail["scope"];
}

/** Syncs one routine mirror through an explicit canonical engine target. */
export interface ArtisanRoutineSyncInput extends MarketplaceSyncRequest {
	readonly command_id?: string;
}

/** Resolves one observed routine provider drift. */
export interface ArtisanRoutineDriftInput extends RoutineDriftResolutionRequest {
	readonly command_id?: string;
}

export interface ArtisanRoutineDriftOverwriteRequestInput extends RoutineDriftOverwriteRequest {
	readonly command_id?: string;
}

export interface ArtisanRoutineDriftOverwriteDecisionInput extends RoutineDriftOverwriteDecision {
	readonly command_id?: string;
}

/** Invokes one eligible routine and returns only ledger-safe metadata. */
export interface ArtisanRoutineInvokeInput extends RoutineInvocationRequest {}

/** Rolls back one routine installation snapshot. */
export interface ArtisanRoutineRollbackInput extends RoutineRollbackRequest {
	readonly command_id?: string;
}

/** Inspects npx-skills candidates without treating their format as canonical. */
export interface ArtisanNpxSkillsDiscoverInput extends NpxSkillsDiscoveryRequest {}

/** Requests canonical import of one previously inspected npx-skills candidate. */
export interface ArtisanNpxSkillsImportInput extends NpxSkillsImportRequest {
	readonly command_id?: string;
}

/** Selects one scope-bound MCP capability without exposing connection handles. */
export interface ArtisanCapabilityDetailInput {
	readonly capability_id: string;
	readonly scope: CapabilityDetail["scope"];
}

/** Binds a capability connection request to an inspected source and transport. */
export interface ArtisanCapabilityConnectInput extends CapabilityConnectPreviewRequest {
	readonly approval_id: string;
	readonly command_id?: string;
	readonly preview_fingerprint: string;
	readonly requested_by: "agent" | "user";
}

/** Resolves the exact inspected MCP connection approval. */
export interface ArtisanCapabilityApprovalInput extends MarketplaceApprovalDecision {
	readonly command_id?: string;
}

/** Carries one capability lifecycle id and an optional durable retry id. */
export interface ArtisanCapabilityIdInput {
	readonly capability_id: string;
	readonly command_id?: string;
	readonly scope: CapabilityDetail["scope"];
}

/** Runs one explicit health check without exposing a socket or process. */
export interface ArtisanCapabilityHealthInput extends CapabilityHealthRequest {
	readonly command_id?: string;
}

/** Syncs one capability mirror through an explicit canonical engine target. */
export interface ArtisanCapabilitySyncInput extends MarketplaceSyncRequest {
	readonly command_id?: string;
}

/** Resolves one observed capability provider drift. */
export interface ArtisanCapabilityDriftInput extends CapabilityDriftResolutionRequest {
	readonly command_id?: string;
}

export interface ArtisanCapabilityDriftOverwriteRequestInput extends CapabilityDriftOverwriteRequest {
	readonly command_id?: string;
}

export interface ArtisanCapabilityDriftOverwriteDecisionInput extends CapabilityDriftOverwriteDecision {
	readonly command_id?: string;
}

/** Invokes one declared MCP tool and returns only ledger-safe metadata. */
export interface ArtisanCapabilityInvokeInput extends CapabilityInvocationRequest {}

export interface ArtisanCapabilityInvocationRequestInput extends CapabilityInvocationApprovalRequest {
	readonly command_id?: string;
}

export interface ArtisanCapabilityInvocationDecisionInput extends CapabilityInvocationApprovalDecision {
	readonly command_id?: string;
}

/** Completes OAuth only through an opaque callback reference. */
export interface ArtisanCapabilityOAuthCompleteInput extends CapabilityOAuthCompleteRequest {
	readonly command_id?: string;
}

/** Starts an OAuth lifecycle action for one capability without exposing token material. */
export interface ArtisanCapabilityOAuthInput extends CapabilityOAuthRequest {
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

/** Delivers the complete Forge-owned project catalog and ordered replacements. */
export type ProjectCatalogUpdate = {
	readonly snapshot: ProjectCatalogSnapshot;
	readonly type: "snapshot" | "replacement";
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
export type ConversationUpdate =
	| { readonly type: "snapshot"; readonly snapshot: ConversationSnapshot }
	| { readonly type: "patch"; readonly batch: ConversationPatchBatch };
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

/** Configures client reconnect timing. */
export interface ArtisanClientOptions {
	/**
	 * Bounds *consecutive* attempts that fail before reaching ready. A session
	 * that becomes ready restores the full budget when it eventually dies, so
	 * only a backend that stays unreachable can exhaust the connection.
	 */
	readonly reconnect_attempts?: number;
	/** Base backoff between failed attempts; growth is capped at 16× the base. */
	readonly reconnect_delay_ms?: number;
}

export type ArtisanConnectionState =
	| { readonly phase: "connecting" }
	| { readonly phase: "reconnecting" }
	| { readonly phase: "ready" }
	| {
			readonly attempts: number;
			readonly error: ArtisanClientError;
			readonly phase: "exhausted";
	  };

/**
 * One structured entry in the client's connection journal. Every connection
 * attempt, negotiation, session death, and published error is recorded here so
 * an intermittent transport failure names its own cause after the fact instead
 * of surfacing only as the last error a reconnect supervisor happened to keep.
 */
export type TransportDiagnosticEvent =
	| {
			readonly at: string;
			readonly kind: "session.attempt";
			/** Counts every attempt over the client's lifetime, healthy or not. */
			readonly ordinal: number;
	  }
	| {
			readonly at: string;
			readonly connection_id: string;
			readonly event_cursor_count: number;
			readonly journal_sequence: number;
			readonly kind: "session.negotiating";
			readonly resume_mode: "fresh" | "resume";
	  }
	| {
			readonly at: string;
			readonly connection_id: string;
			readonly kind: "session.ready";
			readonly negotiation_ms: number;
	  }
	| {
			readonly at: string;
			readonly code: ArtisanClientErrorCode;
			/** Compact rendering of the failure cause, for humans reading a dump. */
			readonly detail: string;
			readonly kind: "session.ended";
			readonly lifetime_ms: number;
			readonly message: string;
			readonly protocol_code: string;
			/** Distinguishes a died session from an attempt that never negotiated. */
			readonly reached_ready: boolean;
	  }
	| {
			readonly at: string;
			readonly code: ArtisanClientErrorCode;
			/**
			 * The session died of a state-integrity failure, so the durable
			 * resume position was dropped and the next attempt bootstraps fresh
			 * instead of resuming into the same divergence forever.
			 */
			readonly kind: "session.resume_dropped";
	  }
	| {
			readonly at: string;
			readonly attempts: number;
			readonly code: ArtisanClientErrorCode;
			readonly kind: "supervisor.exhausted";
			readonly message: string;
			readonly protocol_code: string;
	  }
	| { readonly at: string; readonly kind: "supervisor.retry_released" }
	| {
			readonly at: string;
			readonly code: ArtisanClientErrorCode;
			readonly kind: "error.published";
			readonly message: string;
			readonly protocol_code: string;
	  }
	| { readonly at: string; readonly kind: "client.disposed" };

/** The complete journal of transport events, oldest first. */
export interface TransportDiagnosticsSnapshot {
	/** Retained for wire compatibility; a lossless journal always reports zero. */
	readonly dropped: 0;
	readonly events: ReadonlyArray<TransportDiagnosticEvent>;
}

/**
 * Provides typed frontend operations while hiding protocol envelopes and cursors.
 * MessagePorts are reliable while alive: commands retry only after reconnect,
 * using the exact original envelope and id until a durable receipt arrives.
 */
