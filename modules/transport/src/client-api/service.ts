import { Context, Effect, Option, Scope, Stream } from "effect";

import type {
	CapabilityConnectPreview,
	CapabilityConnectPreviewRequest,
	CapabilityDetail,
	CapabilityInvocationMetadata,
	CapabilityOAuthBeginResult,
	CapabilityOAuthTokenStatus,
	CapabilityRegistrySnapshot,
	ConversationQuery,
	ConversationSnapshot,
	MessageImageAttachment,
	MessageImageAttachmentQuery,
	EventEnvelope,
	GlobalGuidanceSnapshot,
	GitDiffQueryResult,
	GitWorkspaceQueryResult,
	ModelBehaviourSnapshot,
	NpxSkillsDiscoveryResult,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	PreviewAssetId,
	PreviewBrowserLaunch,
	PreviewInspectionResult,
	PreviewInspectionSession,
	PreviewTarget,
	PreviewTargetListQuery,
	RichLinkResolution,
	RichLinkAssetMetadata,
	SurfaceListQuery,
	SurfaceSnapshot,
	SurfaceUsageAggregateQuery,
	SurfaceUsageAggregateSnapshot,
	SurfaceUsageDailyQuery,
	SurfaceUsageDailySnapshot,
	TerminalSession,
	ThreadCreateInput,
	ThreadListItem,
	ModelFavoritesSnapshot,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	RuntimeCatalog,
	HostIdentitySnapshot,
	ThreadUsageSeries,
	ThreadUsageSeriesQuery,
	EngineUsageQuery,
	EngineUsageSnapshot,
	ProjectDiffQueryResult,
	ProjectRepositoryQueryResult,
	SessionDefaults,
	SessionDefaultsUpdateInput,
	ThreadTranscriptQuery,
	ThreadTranscriptSnapshot,
	ThreadSessionSnapshot,
	RoutineDetail,
	RoutineInstallPreview,
	RoutineInstallPreviewRequest,
	RoutineInvocationMetadata,
	RoutineRegistrySnapshot,
	WorkspaceFileReadQueryResult,
	WorkspaceChangeListQueryResult,
	WorkspaceChangeDiffQueryResult,
	ArtisanApprovalListQueryResult,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolRegistryListQueryResult,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceLanguageCapabilitiesQueryResult,
	WorkspaceConflictListQueryResult,
} from "@artisan/protocol";
import type {
	ArtisanApprovalListInput,
	ArtisanApprovalResolveInput,
	ArtisanCapabilityApprovalInput,
	ArtisanCapabilityConnectInput,
	ArtisanCapabilityDetailInput,
	ArtisanCapabilityDriftInput,
	ArtisanCapabilityDriftOverwriteDecisionInput,
	ArtisanCapabilityDriftOverwriteRequestInput,
	ArtisanCapabilityHealthInput,
	ArtisanCapabilityIdInput,
	ArtisanCapabilityInvocationDecisionInput,
	ArtisanCapabilityInvocationRequestInput,
	ArtisanCapabilityInvokeInput,
	ArtisanCapabilityOAuthCompleteInput,
	ArtisanCapabilityOAuthInput,
	ArtisanCapabilitySyncInput,
	ArtisanClientCursors,
	ArtisanClientError,
	ArtisanCommandInput,
	ArtisanCommandReceipt,
	ArtisanConnectionState,
	ArtisanGitDiffInput,
	ArtisanGitIndexMutationInput,
	ArtisanGitMutationResolveInput,
	ArtisanGitWorkspaceInput,
	ArtisanGlobalGuidanceDriftInput,
	ArtisanGlobalGuidanceRetryInput,
	ArtisanGlobalGuidanceSelectionInput,
	ArtisanGlobalGuidanceUpdateInput,
	ArtisanMarketplaceBrowseInput,
	ArtisanModelBehaviourDriftInput,
	ArtisanModelBehaviourRetryInput,
	ArtisanModelBehaviourUpdateInput,
	ArtisanModelFavoriteUpdateInput,
	ArtisanNpxSkillsDiscoverInput,
	ArtisanNpxSkillsImportInput,
	ArtisanPreviewAssetMetadataInput,
	ArtisanPreviewInspectionInput,
	ArtisanPreviewInspectionOpenInput,
	ArtisanPreviewTargetInput,
	ArtisanPreviewTargetRegistrationInput,
	ArtisanPreviewTargetStateInput,
	ArtisanRichLinkResolveInput,
	ArtisanRoutineApprovalInput,
	ArtisanRoutineDetailInput,
	ArtisanRoutineDriftInput,
	ArtisanRoutineDriftOverwriteDecisionInput,
	ArtisanRoutineDriftOverwriteRequestInput,
	ArtisanRoutineIdInput,
	ArtisanRoutineInstallInput,
	ArtisanRoutineInvokeInput,
	ArtisanRoutineRollbackInput,
	ArtisanRoutineSyncInput,
	ArtisanThreadRetentionUpdateInput,
	ArtisanThreadSessionPolicyUpdateInput,
	ArtisanToolExecuteInput,
	ArtisanToolInvocationListInput,
	ArtisanToolRegistryListInput,
	ArtisanWorkspaceChangeDiffInput,
	ArtisanWorkspaceChangeListInput,
	ArtisanWorkspaceChangeReviewInput,
	ArtisanWorkspaceChangeRollbackInput,
	ArtisanWorkspaceFileDiscoveryInput,
	ArtisanWorkspaceFileReadInput,
	ArtisanWorkspaceFileReplaceInput,
	ArtisanWorkspaceLanguageCapabilitiesInput,
	ConversationUpdate,
	OrchestrationGraphUpdate,
	OrchestrationGroupListUpdate,
	ProjectCatalogUpdate,
	SurfaceListUpdate,
	SurfaceUsageAggregateUpdate,
	ThreadListUpdate,
	ThreadSessionUpdate,
	ThreadTranscriptUpdate,
	TransportDiagnosticEvent,
	TransportDiagnosticsSnapshot,
	WorkspaceConflictListUpdate,
} from "./contract";
import type {
	Project,
	ProjectCatalogSnapshot,
	ProjectDirectoryCreateInput,
	ProjectDirectoryEntry,
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectorySelectInput,
} from "@artisan/protocol";

export * from "./contract";

export class ArtisanClient extends Context.Service<
	ArtisanClient,
	{
		readonly Command: (
			input: ArtisanCommandInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly Cursors: Effect.Effect<ArtisanClientCursors>;
		readonly ConnectionChanges: Stream.Stream<ArtisanConnectionState>;
		readonly ConnectionState: Effect.Effect<ArtisanConnectionState>;
		/** The bounded journal of everything the transport has done so far. */
		readonly Diagnostics: Effect.Effect<TransportDiagnosticsSnapshot>;
		/** Live feed of journal entries as they are recorded. */
		readonly DiagnosticEvents: Stream.Stream<TransportDiagnosticEvent>;
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
		readonly GetModelFavorites: Effect.Effect<ModelFavoritesSnapshot, ArtisanClientError>;
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
		readonly GetEngineUsage: (
			input?: EngineUsageQuery,
		) => Effect.Effect<EngineUsageSnapshot, ArtisanClientError>;
		/**
		 * Reads one thread's per-turn token series for the context window it is
		 * currently in. Cut at the last compaction rather than spanning the thread.
		 */
		readonly GetThreadUsageSeries: (
			input: ThreadUsageSeriesQuery,
		) => Effect.Effect<ThreadUsageSeries, ArtisanClientError>;
		/** Reads repository identity for the named projects, or all of them when empty. */
		/** Reads the defaults a new draft inherits; Forge owns them, not the browser. */
		readonly GetSessionDefaults: Effect.Effect<SessionDefaults, ArtisanClientError>;
		readonly UpdateSessionDefaults: (
			input: SessionDefaultsUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly GetProjectRepositories: (
			project_ids?: ReadonlyArray<string>,
		) => Effect.Effect<ProjectRepositoryQueryResult, ArtisanClientError>;
		/** Reads uncommitted diff summaries for the named projects, or all of them when empty. */
		readonly GetProjectDiffs: (
			project_ids?: ReadonlyArray<string>,
		) => Effect.Effect<ProjectDiffQueryResult, ArtisanClientError>;
		readonly DetachProject: (
			project_id: string,
		) => Effect.Effect<ProjectCatalogSnapshot, ArtisanClientError>;
		readonly ListProjectDirectories: (
			input?: ProjectDirectoryListInput,
		) => Effect.Effect<ProjectDirectoryList, ArtisanClientError>;
		readonly SelectProjectDirectory: (
			input: ProjectDirectorySelectInput,
		) => Effect.Effect<Project, ArtisanClientError>;
		readonly CreateProjectDirectory: (
			input: ProjectDirectoryCreateInput,
		) => Effect.Effect<ProjectDirectoryEntry, ArtisanClientError>;
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
		readonly UpdateModelFavorite: (
			input: ArtisanModelFavoriteUpdateInput,
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
