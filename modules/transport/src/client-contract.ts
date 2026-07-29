import { Context, Data, Effect, Option, Scope, Stream } from "effect";

import type {
	CommandPayload,
	CapabilityConnectPreview,
	CapabilityConnectPreviewRequest,
	CapabilityDetail,
	CapabilityDriftResolutionRequest,
	CapabilityDriftOverwriteDecision,
	CapabilityDriftOverwriteRequest,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationMetadata,
	CapabilityInvocationRequest,
	CapabilityOAuthCompleteRequest,
	CapabilityOAuthBeginResult,
	CapabilityOAuthRequest,
	CapabilityOAuthTokenStatus,
	CapabilityRegistrySnapshot,
	CapabilityHealthRequest,
	ConversationPatchBatch,
	ConversationQuery,
	ConversationSnapshot,
	MessageImageAttachment,
	MessageImageAttachmentQuery,
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
	MarketplaceApprovalDecision,
	MarketplaceBrowseQuery,
	MarketplaceSyncRequest,
	NpxSkillsDiscoveryRequest,
	NpxSkillsDiscoveryResult,
	NpxSkillsImportRequest,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	PreviewAssetId,
	PreviewAssetMetadataQuery,
	PreviewBrowserLaunch,
	PreviewInspectionRequest,
	PreviewInspectionResult,
	PreviewInspectionSession,
	PreviewInspectionSessionOpenRequest,
	PreviewTarget,
	PreviewTargetGetQuery,
	PreviewTargetListQuery,
	PreviewTargetRegistration,
	PreviewTargetStateRequest,
	RichLinkResolution,
	RichLinkResolveQuery,
	RichLinkAssetMetadata,
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
	SurfaceUsageDailyQuery,
	SurfaceUsageDailySnapshot,
	StreamCursor,
	TerminalSession,
	ThreadCreateInput,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	RawOrigin,
	RuntimeCatalog,
	HostIdentitySnapshot,
	EngineUsageSnapshot,
	ThreadTranscriptQuery,
	ThreadTranscriptSnapshot,
	ThreadSessionSnapshot,
	ThreadSessionPolicy,
	TranscriptEntry,
	RoutineDetail,
	RoutineDriftResolutionRequest,
	RoutineDriftOverwriteDecision,
	RoutineDriftOverwriteRequest,
	RoutineInstallPreview,
	RoutineInstallPreviewRequest,
	RoutineInstallRequest,
	RoutineInvocationMetadata,
	RoutineInvocationRequest,
	RoutineRegistrySnapshot,
	RoutineRollbackRequest,
	WorkspaceFileReadQuery,
	WorkspaceFileReadQueryResult,
	WorkspaceFileReplaceRequest,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeDiffQuery,
	WorkspaceChangeDiffQueryResult,
	ArtisanApprovalListQuery,
	ArtisanApprovalListQueryResult,
	ArtisanApprovalResolveRequest,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocationListQuery,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolRegistryListQuery,
	ArtisanToolRegistryListQueryResult,
	WorkspaceFileDiscoveryQuery,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceLanguageCapabilitiesQuery,
	WorkspaceLanguageCapabilitiesQueryResult,
	WorkspaceChangeReview,
	WorkspaceConflictListQueryResult,
} from "@artisan/protocol";
import type {
	Project,
	ProjectCatalogSnapshot,
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectorySelectInput,
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
	| "stream_not_found"
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

/** Configures bounded client queues, reconnect timing, and request concurrency. */
export interface ArtisanClientOptions {
	readonly error_capacity?: number;
	readonly event_capacity?: number;
	readonly max_pending_requests?: number;
	readonly reconnect_attempts?: number;
	readonly reconnect_delay_ms?: number;
	readonly stream_capacity?: number;
	readonly subscription_capacity?: number;
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
		readonly ConnectionChanges: Stream.Stream<ArtisanConnectionState>;
		readonly ConnectionState: Effect.Effect<ArtisanConnectionState>;
		readonly Dispose: Effect.Effect<void>;
		readonly Errors: Stream.Stream<ArtisanClientError>;
		readonly Events: Stream.Stream<EventEnvelope, ArtisanClientError>;
		readonly RetryConnection: Effect.Effect<void>;
		readonly GetOrchestrationGraph: (
			group_id: string,
		) => Effect.Effect<OrchestrationGraph, ArtisanClientError>;
		readonly GetConversation: (
			input: ConversationQuery,
		) => Effect.Effect<ConversationSnapshot, ArtisanClientError>;
		readonly GetMessageImageAttachment: (
			input: MessageImageAttachmentQuery,
		) => Effect.Effect<Option.Option<MessageImageAttachment>, ArtisanClientError>;
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
		readonly GetSurfaceUsageDaily: (
			input: SurfaceUsageDailyQuery,
		) => Effect.Effect<SurfaceUsageDailySnapshot, ArtisanClientError>;
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
		readonly ListArtisanApprovals: (
			input: ArtisanApprovalListInput,
		) => Effect.Effect<ArtisanApprovalListQueryResult, ArtisanClientError>;
		readonly ListArtisanToolInvocations: (
			input: ArtisanToolInvocationListInput,
		) => Effect.Effect<ArtisanToolInvocationListQueryResult, ArtisanClientError>;
		readonly ListArtisanTools: (
			input: ArtisanToolRegistryListInput,
		) => Effect.Effect<ArtisanToolRegistryListQueryResult, ArtisanClientError>;
		readonly GetPreviewAssetMetadata: (
			input: ArtisanPreviewAssetMetadataInput,
		) => Effect.Effect<RichLinkAssetMetadata, ArtisanClientError>;
		readonly GetPreviewTarget: (
			input: ArtisanPreviewTargetInput,
		) => Effect.Effect<PreviewTarget, ArtisanClientError>;
		readonly GetRoutineDetail: (
			input: ArtisanRoutineDetailInput,
		) => Effect.Effect<RoutineDetail, ArtisanClientError>;
		readonly GetCapabilityDetail: (
			input: ArtisanCapabilityDetailInput,
		) => Effect.Effect<CapabilityDetail, ArtisanClientError>;
		readonly GetCapabilityOAuthStatus: (
			input: ArtisanCapabilityOAuthInput,
		) => Effect.Effect<CapabilityOAuthTokenStatus, ArtisanClientError>;
		readonly GetThreadRetentionPolicy: Effect.Effect<ThreadRetentionPolicy, ArtisanClientError>;
		readonly GetThreadWork: (
			thread_id: string,
		) => Effect.Effect<Option.Option<ThreadWorkItem>, ArtisanClientError>;
		readonly CreateThread: (
			input: ThreadCreateInput,
		) => Effect.Effect<ThreadListItem, ArtisanClientError>;
		readonly ListTerminals: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, ArtisanClientError>;
		readonly ListThreads: Effect.Effect<ReadonlyArray<ThreadListItem>, ArtisanClientError>;
		readonly ListProjects: Effect.Effect<ProjectCatalogSnapshot, ArtisanClientError>;
		readonly GetRuntimeCatalog: Effect.Effect<RuntimeCatalog, ArtisanClientError>;
		readonly GetHostIdentity: Effect.Effect<HostIdentitySnapshot, ArtisanClientError>;
		readonly GetEngineUsage: Effect.Effect<EngineUsageSnapshot, ArtisanClientError>;
		readonly DetachProject: (
			project_id: string,
		) => Effect.Effect<ProjectCatalogSnapshot, ArtisanClientError>;
		readonly ListProjectDirectories: (
			input?: ProjectDirectoryListInput,
		) => Effect.Effect<ProjectDirectoryList, ArtisanClientError>;
		readonly SelectProjectDirectory: (
			input: ProjectDirectorySelectInput,
		) => Effect.Effect<Project, ArtisanClientError>;
		readonly ListPreviewTargets: (
			input?: PreviewTargetListQuery,
		) => Effect.Effect<ReadonlyArray<PreviewTarget>, ArtisanClientError>;
		readonly ListRoutines: (
			input: ArtisanMarketplaceBrowseInput,
		) => Effect.Effect<RoutineRegistrySnapshot, ArtisanClientError>;
		readonly ListCapabilities: (
			input: ArtisanMarketplaceBrowseInput,
		) => Effect.Effect<CapabilityRegistrySnapshot, ArtisanClientError>;
		readonly ListWorkspaceChanges: (
			input: ArtisanWorkspaceChangeListInput,
		) => Effect.Effect<WorkspaceChangeListQueryResult, ArtisanClientError>;
		readonly ListWorkspaceFiles: (
			input: ArtisanWorkspaceFileDiscoveryInput,
		) => Effect.Effect<WorkspaceFileDiscoveryQueryResult, ArtisanClientError>;
		readonly ListWorkspaceConflicts: (
			thread_id: string,
		) => Effect.Effect<WorkspaceConflictListQueryResult, ArtisanClientError>;
		readonly GetWorkspaceChangeDiff: (
			input: ArtisanWorkspaceChangeDiffInput,
		) => Effect.Effect<WorkspaceChangeDiffQueryResult, ArtisanClientError>;
		readonly GetWorkspaceLanguageCapabilities: (
			input: ArtisanWorkspaceLanguageCapabilitiesInput,
		) => Effect.Effect<WorkspaceLanguageCapabilitiesQueryResult, ArtisanClientError>;
		readonly OpenAsset: (
			asset_id: PreviewAssetId,
		) => Effect.Effect<
			Stream.Stream<Uint8Array, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly OpenTerminalOutput: (input: {
			readonly terminal_id: string;
			readonly thread_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<
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
		readonly SubscribeProjects: Effect.Effect<
			Stream.Stream<ProjectCatalogUpdate, ArtisanClientError>,
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
		readonly ResolveGlobalGuidanceDrift: (
			input: ArtisanGlobalGuidanceDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RequestGitIndexMutation: (
			input: ArtisanGitIndexMutationInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveGitMutation: (
			input: ArtisanGitMutationResolveInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveArtisanApproval: (
			input: ArtisanApprovalResolveInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveModelBehaviourDrift: (
			input: ArtisanModelBehaviourDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ProbePreviewTarget: (
			input: ArtisanPreviewTargetInput,
		) => Effect.Effect<PreviewTarget, ArtisanClientError>;
		readonly RegisterPreviewTarget: (
			input: ArtisanPreviewTargetRegistrationInput,
		) => Effect.Effect<PreviewTarget, ArtisanClientError>;
		readonly RemovePreviewTarget: (
			input: ArtisanPreviewTargetInput,
		) => Effect.Effect<PreviewTarget, ArtisanClientError>;
		readonly ResolveRichLink: (
			input: ArtisanRichLinkResolveInput,
		) => Effect.Effect<RichLinkResolution, ArtisanClientError>;
		readonly LaunchPreviewInExternalBrowser: (
			input: ArtisanPreviewTargetInput,
		) => Effect.Effect<PreviewBrowserLaunch, ArtisanClientError>;
		readonly OpenPreviewInspectionSession: (
			input: ArtisanPreviewInspectionOpenInput,
		) => Effect.Effect<PreviewInspectionSession, ArtisanClientError>;
		readonly InspectPreviewSession: (
			input: ArtisanPreviewInspectionInput,
		) => Effect.Effect<PreviewInspectionResult, ArtisanClientError>;
		readonly ClosePreviewInspectionSession: (
			session_id: string,
		) => Effect.Effect<PreviewInspectionSession, ArtisanClientError>;
		readonly SetPreviewTargetState: (
			input: ArtisanPreviewTargetStateInput,
		) => Effect.Effect<PreviewTarget, ArtisanClientError>;
		readonly PreviewRoutineInstall: (
			input: RoutineInstallPreviewRequest,
		) => Effect.Effect<RoutineInstallPreview, ArtisanClientError>;
		readonly RequestRoutineInstall: (
			input: ArtisanRoutineInstallInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DecideRoutineInstall: (
			input: ArtisanRoutineApprovalInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly EnableRoutine: (
			input: ArtisanRoutineIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DisableRoutine: (
			input: ArtisanRoutineIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RemoveRoutine: (
			input: ArtisanRoutineIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RollbackRoutine: (
			input: ArtisanRoutineRollbackInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly SyncRoutine: (
			input: ArtisanRoutineSyncInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveRoutineDrift: (
			input: ArtisanRoutineDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RequestRoutineDriftOverwrite: (
			input: ArtisanRoutineDriftOverwriteRequestInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DecideRoutineDriftOverwrite: (
			input: ArtisanRoutineDriftOverwriteDecisionInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly InvokeRoutine: (
			input: ArtisanRoutineInvokeInput,
		) => Effect.Effect<RoutineInvocationMetadata, ArtisanClientError>;
		readonly DiscoverNpxSkills: (
			input: ArtisanNpxSkillsDiscoverInput,
		) => Effect.Effect<NpxSkillsDiscoveryResult, ArtisanClientError>;
		readonly ImportNpxSkills: (
			input: ArtisanNpxSkillsImportInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly PreviewCapabilityConnect: (
			input: CapabilityConnectPreviewRequest,
		) => Effect.Effect<CapabilityConnectPreview, ArtisanClientError>;
		readonly RequestCapabilityConnect: (
			input: ArtisanCapabilityConnectInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DecideCapabilityConnect: (
			input: ArtisanCapabilityApprovalInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly StartCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ReconnectCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly CheckCapabilityHealth: (
			input: ArtisanCapabilityHealthInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DisconnectCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RestartCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UninstallCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly EnableCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DisableCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RemoveCapability: (
			input: ArtisanCapabilityIdInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly SyncCapability: (
			input: ArtisanCapabilitySyncInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ResolveCapabilityDrift: (
			input: ArtisanCapabilityDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RequestCapabilityDriftOverwrite: (
			input: ArtisanCapabilityDriftOverwriteRequestInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly DecideCapabilityDriftOverwrite: (
			input: ArtisanCapabilityDriftOverwriteDecisionInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RequestCapabilityInvocation: (
			input: ArtisanCapabilityInvocationRequestInput,
		) => Effect.Effect<CapabilityInvocationMetadata, ArtisanClientError>;
		readonly DecideCapabilityInvocation: (
			input: ArtisanCapabilityInvocationDecisionInput,
		) => Effect.Effect<CapabilityInvocationMetadata, ArtisanClientError>;
		readonly InvokeCapability: (
			input: ArtisanCapabilityInvokeInput,
		) => Effect.Effect<CapabilityInvocationMetadata, ArtisanClientError>;
		readonly BeginCapabilityOAuth: (
			input: ArtisanCapabilityOAuthInput,
		) => Effect.Effect<CapabilityOAuthBeginResult, ArtisanClientError>;
		readonly CompleteCapabilityOAuth: (
			input: ArtisanCapabilityOAuthCompleteInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RefreshCapabilityOAuth: (
			input: ArtisanCapabilityOAuthInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RevokeCapabilityOAuth: (
			input: ArtisanCapabilityOAuthInput,
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
		readonly UpdateThreadSessionPolicy: (
			input: ArtisanThreadSessionPolicyUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ReplaceWorkspaceFile: (
			input: ArtisanWorkspaceFileReplaceInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ExecuteArtisanTool: (
			input: ArtisanToolExecuteInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly ReviewWorkspaceChange: (
			input: ArtisanWorkspaceChangeReviewInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RollbackWorkspaceChange: (
			input: ArtisanWorkspaceChangeRollbackInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
	}
>()("Artisan/ArtisanClient") {}
