export {
	DecodeProtocolConnectionOptions,
	DefaultProtocolConnectionOptions,
	type ProtocolConnection,
	type ProtocolConnectionOptions,
	ProtocolConfigurationError,
	ProtocolConnectionOptionsSchema,
} from "./protocol/protocol-connection";
export { ProtocolRouter } from "./protocol/protocol-router";
export { ProtocolServer } from "./protocol/protocol-server";
export {
	GlobalGuidanceConflict,
	GlobalGuidanceInvariantError,
	GlobalGuidanceService,
	make_global_guidance_service_layer,
	type GlobalGuidanceMutationResult,
	type GlobalGuidanceMutationTrace,
	type GlobalGuidanceServiceError,
	type GlobalGuidanceServiceOptions,
} from "./guidance/guidance-service";
export {
	EmptyGuidanceProviderRegistryLive,
	GuidanceProviderRegistry,
	make_claude_guidance_adapter,
	make_codex_guidance_adapter,
	make_guidance_provider_registry_layer,
	make_platform_guidance_provider_registry_layer,
	make_runtime_guidance_adapter,
	make_unsupported_guidance_adapter,
} from "./guidance/provider-mirrors";
export {
	CodexModelBehaviourProbe,
	make_codex_model_behaviour_probe_layer,
	type CodexModelBehaviourProbeAvailable,
	type CodexModelBehaviourProbeOptions,
	type CodexModelBehaviourProbeResult,
	type CodexModelBehaviourProbeUnavailable,
} from "./model-behaviour/codex-probe";
export {
	codex_auto_compaction_native_key,
	CodexModelBehaviourConfigError,
	patch_codex_model_behaviour,
	read_codex_model_behaviour,
	type CodexModelBehaviourValue,
} from "./model-behaviour/codex-config";
export {
	make_model_behaviour_config_files_layer,
	ModelBehaviourConfigFileBackupError,
	ModelBehaviourConfigFileReadError,
	ModelBehaviourConfigFileReplaceError,
	ModelBehaviourConfigFileRestoreError,
	ModelBehaviourConfigFiles,
	ModelBehaviourConfigFilesLive,
	ModelBehaviourConfigFileWriteError,
	type ModelBehaviourConfigFileHooks,
	type ModelBehaviourConfigFileReplaceOptions,
	type ModelBehaviourConfigFileReplaceResult,
	type ModelBehaviourConfigFileSnapshot,
} from "./model-behaviour/model-behaviour-config-files";
export {
	EmptyModelBehaviourProviderRegistryLive,
	make_codex_model_behaviour_provider,
	make_desktop_model_behaviour_provider_registry_layer,
	make_inactive_model_behaviour_provider,
	make_model_behaviour_provider_registry_layer,
	ModelBehaviourProviderError,
	ModelBehaviourProviderRegistry,
	type DesktopModelBehaviourProviderOptions,
	type ModelBehaviourProviderAdapter,
	type ModelBehaviourProviderApplyInput,
	type ModelBehaviourProviderApplyResult,
	type ModelBehaviourProviderErrorCode,
	type ModelBehaviourProviderObservation,
} from "./model-behaviour/model-behaviour-provider";
export {
	BuildModelBehaviourCapabilities,
	make_codex_auto_compaction_mapping,
	make_model_behaviour_capability_registry_layer,
	make_unavailable_auto_compaction_mapping,
	make_unsupported_auto_compaction_mapping,
	ModelBehaviourCapabilityRegistry,
	ModelBehaviourRegistryError,
	type ModelBehaviourProviderMapping,
} from "./model-behaviour/model-behaviour-registry";
export {
	model_behaviour_thread_id,
	ModelBehaviourRepository,
	ModelBehaviourRepositoryLive,
	type ModelBehaviourCommit,
	type ModelBehaviourCommitResult,
	type ModelBehaviourEvent,
	type ModelBehaviourOperation,
	type ModelBehaviourPreflight,
	type ModelBehaviourProviderCommit,
	type ModelBehaviourReadResult,
	type ModelBehaviourRepositoryError,
} from "./model-behaviour/model-behaviour-repository";
export {
	ModelBehaviourConflict,
	ModelBehaviourInvariantError,
	ModelBehaviourService,
	ModelBehaviourServiceLive,
	type ModelBehaviourMutationResult,
	type ModelBehaviourMutationTrace,
	type ModelBehaviourServiceError,
} from "./model-behaviour/model-behaviour-service";
export { hash_model_behaviour_value } from "./model-behaviour/model-behaviour-value";
export { ThreadErasure, ThreadErasureFailure } from "./threads/thread-erasure";
export {
	ThreadRetention,
	ThreadRetentionClock,
	ThreadRetentionFailure,
	ThreadRetentionScheduler,
} from "./threads/thread-retention";
export {
	ThreadResourceQuiescer,
	ThreadResourceQuiescenceFailure,
} from "./threads/thread-resource-quiescer";
export {
	ThreadMetadataRefinementCoordinator,
	ThreadMetadataRefinementCoordinatorDisabled,
	ThreadMetadataRefinementCoordinatorLive,
	ThreadMetadataRefinementPending,
	type ThreadMetadataRefinementCoordinatorError,
} from "./threads/thread-metadata-refinement-coordinator";
export {
	make_thread_metadata_refinement_worker_layer,
	ThreadMetadataRefinementWorker,
	type ThreadMetadataRefinementSubmission,
	type ThreadMetadataRefinementWorkerOptions,
} from "./threads/thread-metadata-refinement-worker";
export {
	bound_thread_metadata_refiner_input,
	make_thread_metadata_refiner_test_layer,
	ThreadMetadataRefiner,
	ThreadMetadataRefinerLive,
	type ThreadMetadataRefinement,
	type ThreadMetadataRefinementRequest,
	type ThreadMetadataRefinementTrigger,
	type ThreadMetadataRefinerInput,
} from "./threads/thread-metadata-refiner";
export {
	ThreadMetadataRefinementIntent,
	ThreadMetadataRepository,
	type ThreadMetadataAcceptance,
	type ThreadMetadataError,
} from "./threads/thread-metadata-repository";
export {
	make_node_project_locator_layer,
	EmptyProjectLocatorLive,
	ProjectLocator,
	ProjectLocatorError,
	type ProjectLocation,
	type ProjectLocationSource,
	type ProjectLocatorOperation,
} from "./threads/project-locator";
export {
	ThreadProjectAffinityEvidenceInput,
	ThreadProjectAffinityNotFound,
	ThreadProjectAffinityRepository,
	ThreadProjectAffinityRepositoryLive,
	type ThreadProjectAffinityAcceptance,
	type ThreadProjectAffinityError,
} from "./threads/thread-project-affinity-repository";
export {
	ThreadProjectAffinityCoordinator,
	ThreadProjectAffinityCoordinatorDisabled,
	ThreadProjectAffinityCoordinatorLive,
	type ThreadProjectAffinityCoordinatorError,
} from "./threads/thread-project-affinity-coordinator";
export { AgentOrchestrator } from "./orchestration/agent-orchestrator";
export {
	AgentGraphOrchestrator,
	AgentGraphOrchestratorLive,
} from "./orchestration/agent-graph-orchestrator";
export {
	AgentGraphCommandConflict,
	AgentGraphFailure,
	AgentGraphInvalid,
	AgentGraphNotFound,
	AgentGraphRepository,
	AgentGraphRepositoryLive,
	type AcceptedAgentGraphCommand,
	type AgentGraphControlClaim,
	type AgentGraphError,
	type PendingAgentRun,
} from "./orchestration/agent-graph-repository";
export {
	Filesystem,
	FilesystemError,
	type FilesystemChange,
	type FilesystemEntry,
	type FilesystemEntryKind,
	type FilesystemOperation,
	type FilesystemPathChange,
	type FilesystemWatchOverflow,
} from "./filesystem/filesystem";
export { make_node_filesystem_layer } from "./filesystem/node-filesystem";
export {
	Git,
	GitError,
	type GitDiffPatch,
	type GitDiffStats,
	type GitFileSummary,
	type GitOperation,
	type GitRepository,
} from "./git/git";
export { make_git_layer, make_node_git_layer, type NodeGitOptions } from "./git/node-git";
export {
	make_node_process_runner_layer,
	NodeProcessRunnerLive,
	type NodeProcessRunnerOptions,
} from "./git/node-process-runner";
export {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerOperation,
	type ProcessRunnerResult,
	type ProcessRunnerShape,
} from "./git/process-runner";
export {
	PreviewHealthProbe,
	PreviewHealthProbeError,
	PreviewTarget,
	PreviewTargetClock,
	PreviewTargetError,
	type PreviewHealthProbeResult,
	type PreviewTargetErrorCode,
	type PreviewTargetEvent,
	type PreviewTargetHealth,
	type PreviewTargetRecord,
	type PreviewTargetRegistration,
	type PreviewTargetSource,
	type PreviewTargetState,
} from "./preview/preview-target";
export {
	make_preview_target_layer,
	PreviewTargetClockLive,
	type PreviewTargetOptions,
} from "./preview/preview-target-service";
export {
	RichLinkAssetStore,
	RichLinkAssetStoreError,
	RichLinkAssetStoreLive,
	make_in_memory_rich_link_asset_store_layer,
	type RichLinkAsset,
	type RichLinkAssetMetadata,
	type RichLinkAssetStoreErrorCode,
	type RichLinkAssetStoreInput,
	type RichLinkAssetStoreLimits,
	type RichLinkAssetStoreOptions,
} from "./preview/rich-link-asset-store";
export {
	RichLinkClock,
	RichLinkDnsError,
	RichLinkDnsResolver,
	RichLinkHttpTransport,
	RichLinkMetadata,
	RichLinkMetadataCache,
	RichLinkMetadataError,
	RichLinkTransportError,
	type RichLinkCacheEntry,
	type RichLinkCacheMetadata,
	type RichLinkFavicon,
	type RichLinkHttpRequest,
	type RichLinkHttpResponse,
	type RichLinkMetadataDocument,
	type RichLinkMetadataErrorCode,
	type RichLinkMetadataResult,
	type RichLinkResolvedAddress,
	type RichLinkTransportErrorCode,
} from "./preview/rich-link-metadata";
export {
	make_node_rich_link_metadata_layer,
	make_rich_link_metadata_layer,
	type NodeRichLinkMetadataOptions,
	type RichLinkMetadataOptions,
} from "./preview/rich-link-service";
export {
	NodePtyTerminalDriverLive,
	make_node_pty_terminal_driver_layer,
	type NodePtyTerminalDriverOptions,
} from "./terminal/node-pty-terminal-driver";
export {
	TerminalDriver,
	TerminalDriverError,
	type TerminalDriverExit,
	type TerminalDriverHandle,
	type TerminalDriverOpenInput,
	type TerminalDriverOperation,
} from "./terminal/terminal-driver";
export {
	TerminalSessionService,
	TerminalSessionServiceLive,
	type TerminalCommandAcceptance,
	type TerminalSessionError,
} from "./terminal/terminal-sessions";
export {
	TerminalCommandConflict,
	TerminalInvariantError,
	TerminalNotActive,
	TerminalNotFound,
	TerminalPersistenceFailure,
	TerminalRepository,
	TerminalRepositoryLive,
	type TerminalRepositoryError,
} from "./terminal/terminal-repository";
export {
	make_backend_layer,
	make_backend_runtime,
	make_desktop_backend_layer,
	make_desktop_backend_runtime,
	type BackendOptions,
	type DesktopBackendOptions,
	type DesktopGuidanceOptions,
	type DesktopModelBehaviourOptions,
} from "./runtime/backend-runtime";
