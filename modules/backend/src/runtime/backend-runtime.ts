import { dirname, join } from "node:path";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient";

import {
	type Engine,
	EngineProcessFactoryLive,
	make_engine_registry_layer,
} from "@artisan/engines";
import { MakeSnowflakeIdLive } from "@artisan/protocol";

import { AgentGraphOrchestratorLive } from "../orchestration/agent-graph-orchestrator";
import { AgentNameCatalogLive } from "../orchestration/agent-name-catalog";
import { AgentGraphRepositoryLive } from "../orchestration/agent-graph-repository";
import { HostSuspendMonitorLive } from "../host/suspend-monitor";
import {
	MakeSystemWakeLockLayer,
	SystemWakeLockDisabled,
	type SystemWakeLockBackend,
} from "../host/wake-lock";
import { make_platform_wake_lock_backend_layer } from "../host/wake-lock-platform";
import { AgentOrchestratorLive } from "../orchestration/agent-orchestrator";
import { IntakePolicyLive } from "../orchestration/intake-policy";
import { ProductInstructionsLive } from "../orchestration/product-instructions";
import { ThreadContinuationCompactorLive } from "../orchestration/thread-continuation-compactor";
import { ThreadContinuationServiceLive } from "../orchestration/thread-continuation-service";
import { SurfaceServiceLive } from "../surfaces/service";
import {
	make_node_workspace_filesystem_registry_layer,
	WorkspaceFilesystemRegistrationError,
	WorkspaceFilesystemRegistry,
} from "../filesystem/workspace-filesystem-registry";
import {
	NodeWorkspaceBoundedRegularFileStoreRegistryLive,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../filesystem/workspace-bounded-regular-file-store-registry";
import { NodeProcessRunnerLive } from "../git/node-process-runner";
import { GitMutationDriverLive } from "../git/mutation-driver";
import { GitReadServiceLive } from "../git/read-service";
import { GitRepositoryLive } from "../git/repository";
import { GitService, GitServiceLive } from "../git/service";
import {
	make_node_workspace_git_registry_layer,
	WorkspaceGitRegistrationError,
	WorkspaceGitRegistry,
} from "../git/workspace-git-registry";
import { make_database_layer } from "../persistence/database";
import { JournalNotifierLive } from "../persistence/journal-notifier";
import { JournalStoreLive } from "../persistence/journal-store";
import { OrchestrationRepositoryLive } from "../persistence/orchestration/repository";
import { ThreadContinuationRepositoryLive } from "../persistence/thread-continuation/repository";
import { UsageInterruptionServiceLive } from "../persistence/usage-interruption/service";
import { ThreadReadModelLive } from "../persistence/thread-read-model";
import {
	ProjectionRebuildBarrierLive,
	ProjectionRebuildServiceLive,
} from "../persistence/projection-rebuild-service";
import { TranscriptReadModelLive } from "../persistence/transcript-read-model";
import { ConversationReadModelLive } from "../conversation/index.ts";
import { CommandRouterLive } from "../protocol/command-router";
import {
	DefaultProtocolConnectionOptions,
	type ProtocolConnectionOptions,
} from "../protocol/connection";
import { ProtocolRouterLive } from "../protocol/router";
import { make_protocol_server_layer } from "../protocol/server";
import { ThreadCommandsLive } from "../threads/thread-commands";
import { ThreadErasureLive } from "../threads/thread-erasure";
import { ThreadMetadataRepositoryLive } from "../threads/thread-metadata-repository";
import {
	ThreadMetadataRefinementCoordinatorDisabled,
	ThreadMetadataRefinementCoordinatorLive,
} from "../threads/thread-metadata-refinement-coordinator";
import { make_thread_metadata_refinement_worker_layer } from "../threads/thread-metadata-refinement-worker";
import {
	ThreadMetadataRefiner,
	ThreadMetadataRefinerLive,
} from "../threads/thread-metadata-refiner";
import { make_node_project_locator_layer, ProjectLocator } from "../threads/project-locator";
import {
	make_project_directory_service_layer,
	ProjectDirectoryService,
} from "../projects/project-directory-service";
import {
	NativeDirectoryPicker,
	NativeDirectoryPickerUnavailable,
} from "../projects/native-directory-picker";
import { make_node_native_directory_picker_layer } from "../projects/node-native-directory-picker";
import { ProjectCatalogLive } from "../projects/project-catalog";
import {
	ProjectIdentityRegistry,
	ProjectIdentityRegistryLive,
} from "../projects/project-identity-registry";
import { ProjectWorkspaceBindingLive } from "../workspace/projects";
import {
	ThreadProjectAffinityCoordinatorDisabled,
	ThreadProjectAffinityCoordinatorLive,
} from "../threads/thread-project-affinity-coordinator";
import { ThreadProjectAffinityRepositoryLive } from "../threads/thread-project-affinity-repository";
import {
	ThreadResourceQuiescer,
	ThreadResourceQuiescerLive,
} from "../threads/thread-resource-quiescer";
import { ModelFavoritesServiceLive } from "../model-favorites/service";
import { SessionDefaultsServiceLive } from "../settings/session-defaults-service";
import { ThreadRetentionPolicyServiceLive } from "../threads/thread-retention-policy";
import {
	ThreadRetentionClock,
	ThreadRetentionClockLive,
	ThreadRetentionLive,
	ThreadRetentionScheduler,
	ThreadRetentionSchedulerLive,
} from "../threads/thread-retention";
import { NodePtyTerminalDriverLive } from "../terminal/node-pty-driver";
import { TerminalDriver } from "../terminal/driver";
import { TerminalRepositoryLive } from "../terminal/repository";
import { TerminalSessionServiceLive } from "../terminal/sessions";
import { RuntimeMetadata, RuntimeMetadataLive } from "./metadata";
import { RuntimeCatalogLive } from "./catalog";
import {
	DesktopEngineConfigurationError,
	ResolveBackendRuntimeConfiguration,
} from "./backend-runtime-config";
import { GlobalGuidanceRepositoryLive } from "../guidance/repository";
import {
	make_global_guidance_service_layer,
	type GlobalGuidanceServiceOptions,
} from "../guidance/service";
import { GuidanceFileStoreLive } from "../guidance/file-store";
import {
	EmptyGuidanceProviderRegistryLive,
	GuidanceProviderRegistry,
	make_platform_guidance_provider_registry_layer,
} from "../guidance/provider-mirrors";
import { ConfigFileStoreLive } from "../harness-config/file-store";
import {
	EmptyHarnessConfigRegistryLive,
	HarnessConfigRegistry,
	MakeHarnessConfigRegistryLayer,
} from "../harness-config/keys";
import { HarnessConfigLive } from "../harness-config/service";
import { make_codex_model_behaviour_probe_layer } from "../model-behaviour/codex-probe";
import { ModelBehaviourConfigFilesLive } from "../model-behaviour/config-files";
import {
	EmptyModelBehaviourProviderRegistryLive,
	make_desktop_model_behaviour_provider_registry_layer,
	ModelBehaviourProviderRegistry,
} from "../model-behaviour/provider";
import { ModelBehaviourRepositoryLive } from "../model-behaviour/repository";
import { ModelBehaviourRegistryError } from "../model-behaviour/registry";
import { ModelBehaviourServiceLive } from "../model-behaviour/service";
import { WorkspaceChangeRepositoryLive } from "../workspace/changes/repository";
import { WorkspaceEvidenceRecorderLive } from "../workspace/evidence";
import { WorkspaceFileServiceLive } from "../workspace/files/service";
import { WorkspaceMutationAuthorityLive } from "../workspace/mutations/authority";
import { WorkspaceSnapshotStoreLive } from "../workspace/snapshot-store";
import { WorkspaceMutationPayloadStoreLive } from "../workspace/mutations/payloads";
import { WorkspaceChangeDiffServiceLive } from "../workspace/changes/diff";
import { ArtisanToolApprovalPolicyLive } from "../tools/approval-policy";
import { make_artisan_tool_registry_layer } from "../tools/artisan-tool-registry";
import { ArtisanBuiltInToolCapabilityStateLive } from "../tools/builtin-tool-capabilities";
import { ToolInvocationRepositoryLive } from "../tools/tool-invocation-repository";
import { ExecuteToolLive } from "../tools/tool-handlers";
import { ToolControlPlaneLive } from "../tools/tool-control-plane";
import { WorkspaceFileDiscoveryLive } from "../workspace/files/discovery";
import { NodePreviewHealthProbeLive } from "../preview/node-preview-health-probe";
import { NodePreviewExternalBrowserLive } from "../preview/node-preview-runtime";
import { PreviewCoordinatorLive } from "../preview/coordinator";
import {
	PreviewExternalBrowser,
	PreviewInspectionConnector,
	PreviewInspectionConnectorUnavailableLive,
	make_preview_inspection_layer,
} from "../preview/runtime";
import { PreviewRepositoryLive } from "../preview/repository";
import { PreviewServiceLive } from "../preview/service";
import { PreviewTargetClockLive } from "../preview/target-service";
import { make_preview_target_layer } from "../preview/target-service";
import { make_node_rich_link_metadata_layer } from "../preview/rich-link-service";
import { RichLinkAssetStore } from "../preview/rich-link-asset-store";
import { RichLinkMetadata } from "../preview/rich-link-metadata";
import { PreviewHealthProbe } from "../preview/target";
import { CapabilityRepositoryLive } from "../marketplace/capabilities/repository";
import {
	CapabilityOAuthLifecycleLive,
	CapabilityServiceLive,
} from "../marketplace/capabilities/service";
import {
	CapabilityTransportRegistry,
	CapabilityTransportRegistryLive,
	EmptyCapabilityTransportRegistryLive,
} from "../marketplace/capabilities/mcp-transport";
import { EffectHttpMcpDriverLive } from "../marketplace/capabilities/http-transport";
import { EmptySecretStoreLive, SecretStore } from "../marketplace/capabilities/secret-store";
import { EngineProcessStdioMcpDriverLive } from "../marketplace/capabilities/stdio-transport";
import {
	EmptyOAuthAdapterLive,
	make_oauth_layer,
	OAuthAdapter,
} from "../marketplace/capabilities/oauth";
import {
	CapabilityProviderMirror,
	EmptyCapabilityProviderMirrorLive,
	CapabilityMirrorServiceLive,
} from "../marketplace/capabilities/provider-mirrors";
import { RoutineRepositoryLive } from "../marketplace/routines/repository";
import { RoutineServiceLive } from "../marketplace/routines/service";
import {
	EmptyRoutineMirrorRegistryLive,
	NpxSkillsAdapter,
	RoutineInstaller,
	RoutineInstallerError,
	RoutineInspectorError,
	RoutineMirrorRegistry,
	RoutineSourceInspector,
} from "../marketplace/routines/adapters";
import {
	make_local_routine_installer_layer,
	make_local_routine_source_inspector_layer,
	make_npx_skills_process_adapter_layer,
	type NpxSkillsProcessOptions,
} from "../marketplace/routines/production-adapters";

export interface BackendOptions {
	readonly capability_provider_mirror?: Layer.Layer<CapabilityProviderMirror>;
	/** Explicit, inert-until-connect transport selection for reviewed MCP capabilities. */
	readonly capability_transport_registry?: Layer.Layer<CapabilityTransportRegistry>;
	readonly database_path: string;
	readonly engines?: ReadonlyArray<Engine>;
	readonly guidance?: Partial<GlobalGuidanceServiceOptions>;
	readonly guidance_provider_registry?: Layer.Layer<GuidanceProviderRegistry>;
	readonly git_service?: Layer.Layer<GitService>;
	readonly migrations_path: string;
	readonly harness_config_registry?: Layer.Layer<HarnessConfigRegistry>;
	readonly model_behaviour_provider_registry?: Layer.Layer<
		ModelBehaviourProviderRegistry,
		ModelBehaviourRegistryError
	>;
	readonly npx_skills_adapter?: Layer.Layer<NpxSkillsAdapter>;
	readonly oauth_adapter?: Layer.Layer<OAuthAdapter>;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
	readonly preview_external_browser?: Layer.Layer<PreviewExternalBrowser>;
	readonly preview_health_probe?: Layer.Layer<PreviewHealthProbe>;
	readonly preview_inspection_connector?: Layer.Layer<PreviewInspectionConnector>;
	readonly preview_rich_links?: Layer.Layer<RichLinkMetadata | RichLinkAssetStore>;
	readonly project_locator?: Layer.Layer<ProjectLocator, never, ProjectIdentityRegistry>;
	readonly project_directory_roots?: ReadonlyArray<string>;
	readonly project_directory_service?: Layer.Layer<ProjectDirectoryService>;
	readonly native_directory_picker?: Layer.Layer<NativeDirectoryPicker>;
	readonly retention_clock?: Layer.Layer<ThreadRetentionClock>;
	readonly retention_scheduler?: Layer.Layer<ThreadRetentionScheduler>;
	readonly routine_installer?: Layer.Layer<RoutineInstaller>;
	readonly routine_mirror_registry?: Layer.Layer<RoutineMirrorRegistry>;
	readonly routine_source_inspector?: Layer.Layer<RoutineSourceInspector>;
	readonly runtime_metadata?: Layer.Layer<RuntimeMetadata>;
	readonly secret_store?: Layer.Layer<SecretStore>;
	readonly terminal_driver?: Layer.Layer<TerminalDriver>;
	readonly thread_metadata_refiner?: Layer.Layer<ThreadMetadataRefiner>;
	/** Platform power-assertion seam; omitted means the wake lock stays inert. */
	readonly wake_lock_backend?: Layer.Layer<SystemWakeLockBackend>;
	readonly thread_resource_quiescer?: Layer.Layer<ThreadResourceQuiescer>;
	readonly workspace_filesystem_registry?: Layer.Layer<
		WorkspaceFilesystemRegistry,
		WorkspaceFilesystemRegistrationError
	>;
	readonly workspace_git_registry?: Layer.Layer<
		WorkspaceGitRegistry,
		WorkspaceGitRegistrationError
	>;
	readonly workspace_bounded_regular_file_store_registry?: Layer.Layer<WorkspaceBoundedRegularFileStoreRegistry>;
}

/** Configures platform-native provider discovery for the production desktop composition. */
export interface DesktopGuidanceOptions {
	readonly codex_home?: string;
	readonly home_directory?: string;
}

/** Configures provider-native Model Behaviour discovery for desktop composition. */
export interface DesktopModelBehaviourOptions {
	readonly backups_directory?: string;
	readonly codex_command?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
}

/** Configures where declared harness config keys are written on this desktop. */
export interface DesktopHarnessConfigOptions {
	readonly backups_directory?: string;
	readonly claude_home?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
}

/** Extends the portable backend options with desktop provider-path discovery. */
export interface DesktopBackendOptions extends BackendOptions {
	readonly guidance_platform?: DesktopGuidanceOptions;
	readonly harness_config_platform?: DesktopHarnessConfigOptions;
	readonly model_behaviour_platform?: DesktopModelBehaviourOptions;
	/** Explicit installed `npx skills` argv; omitted means discovery fails closed. */
	readonly npx_skills_process?: NpxSkillsProcessOptions;
}

export function make_backend_layer(options: BackendOptions) {
	const protocol_options: ProtocolConnectionOptions = {
		...DefaultProtocolConnectionOptions,
		...options.protocol,
	};
	const domain_ids = MakeSnowflakeIdLive(1).pipe(Layer.orDie);
	const infrastructure = Layer.mergeAll(
		make_database_layer(options),
		options.runtime_metadata ?? RuntimeMetadataLive,
		JournalNotifierLive,
		domain_ids,
	);
	const projection_rebuild = ProjectionRebuildServiceLive.pipe(
		Layer.provideMerge(ProjectionRebuildBarrierLive),
		Layer.provideMerge(infrastructure),
	);
	const persistence = Layer.mergeAll(
		JournalStoreLive,
		OrchestrationRepositoryLive,
		ThreadContinuationRepositoryLive,
		ThreadReadModelLive,
		TranscriptReadModelLive,
		ConversationReadModelLive,
	).pipe(Layer.provideMerge(NodeCrypto.layer), Layer.provideMerge(infrastructure));
	const workspace_evidence = WorkspaceEvidenceRecorderLive.pipe(Layer.provideMerge(persistence));
	const workspace_changes = WorkspaceChangeRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const workspace_snapshots = WorkspaceSnapshotStoreLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const workspace_mutation_payloads = WorkspaceMutationPayloadStoreLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const workspace_diffs = WorkspaceChangeDiffServiceLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const workspace_filesystems =
		options.workspace_filesystem_registry ?? make_node_workspace_filesystem_registry_layer([]);
	const workspace_git_registry =
		options.workspace_git_registry ?? make_node_workspace_git_registry_layer([]);
	/**
	 * Node-backed rather than inert: an attached project is a real directory,
	 * and the project binding below registers it so file reads resolve.
	 */
	const workspace_bounded_filesystems =
		options.workspace_bounded_regular_file_store_registry ??
		NodeWorkspaceBoundedRegularFileStoreRegistryLive;
	const workspace_authority = WorkspaceMutationAuthorityLive.pipe(
		Layer.provideMerge(workspace_bounded_filesystems),
		Layer.provideMerge(workspace_changes),
		Layer.provideMerge(infrastructure),
	);
	const workspace_files = WorkspaceFileServiceLive.pipe(
		Layer.provide(
			Layer.mergeAll(
				NodeCrypto.layer,
				workspace_authority,
				workspace_bounded_filesystems,
				workspace_changes,
				workspace_evidence,
				workspace_diffs,
				workspace_mutation_payloads,
				workspace_snapshots,
			),
		),
	);
	const git_reads = GitReadServiceLive.pipe(
		Layer.provideMerge(workspace_git_registry),
		Layer.provideMerge(NodeCrypto.layer),
	);
	const git_mutations = GitMutationDriverLive.pipe(Layer.provideMerge(workspace_git_registry));
	const git_repository = GitRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
		Layer.provideMerge(NodeCrypto.layer),
	);
	const git =
		options.git_service ??
		GitServiceLive.pipe(
			Layer.provide(
				Layer.mergeAll(
					NodeCrypto.layer,
					git_mutations,
					git_reads,
					git_repository,
					workspace_evidence,
					infrastructure,
				),
			),
		);
	const engine_registry = make_engine_registry_layer(options.engines ?? []);
	const runtime_catalog = RuntimeCatalogLive.pipe(Layer.provideMerge(engine_registry));
	const guidance_repository = GlobalGuidanceRepositoryLive.pipe(Layer.provideMerge(persistence));
	const guidance_directory = join(dirname(options.database_path), "guidance");
	const guidance = make_global_guidance_service_layer({
		backups_directory:
			options.guidance?.backups_directory ?? join(guidance_directory, "backups"),
		canonical_path: options.guidance?.canonical_path ?? join(guidance_directory, "GLOBAL.md"),
	}).pipe(
		Layer.provideMerge(guidance_repository),
		Layer.provideMerge(GuidanceFileStoreLive),
		Layer.provideMerge(options.guidance_provider_registry ?? EmptyGuidanceProviderRegistryLive),
		Layer.provideMerge(infrastructure),
	);
	const model_behaviour_repository = ModelBehaviourRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const model_behaviour = ModelBehaviourServiceLive.pipe(
		Layer.provideMerge(model_behaviour_repository),
		Layer.provideMerge(
			options.model_behaviour_provider_registry ?? EmptyModelBehaviourProviderRegistryLive,
		),
		Layer.provideMerge(infrastructure),
	);
	/**
	 * Harness config writing fails closed: without a registry supplying real
	 * targets, every declared key resolves to an unavailable harness and no
	 * provider file can be touched.
	 */
	const harness_config = HarnessConfigLive.pipe(
		Layer.provideMerge(options.harness_config_registry ?? EmptyHarnessConfigRegistryLive),
		Layer.provideMerge(ConfigFileStoreLive),
	);
	const session_defaults = SessionDefaultsServiceLive.pipe(Layer.provideMerge(persistence));
	const continuation_compactor = ThreadContinuationCompactorLive.pipe(
		Layer.provideMerge(session_defaults),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(infrastructure),
	);
	const continuation = ThreadContinuationServiceLive.pipe(
		Layer.provideMerge(continuation_compactor),
		Layer.provideMerge(persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const usage_interruptions = UsageInterruptionServiceLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(runtime_catalog),
	);
	const agent_name_catalog = AgentNameCatalogLive.pipe(Layer.provideMerge(session_defaults));
	const graph_persistence = AgentGraphRepositoryLive.pipe(
		Layer.provideMerge(agent_name_catalog),
		Layer.provideMerge(infrastructure),
	);
	const orchestration = AgentOrchestratorLive.pipe(
		Layer.provide(graph_persistence),
		Layer.provideMerge(persistence),
		Layer.provideMerge(continuation),
		Layer.provideMerge(usage_interruptions),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(guidance),
		Layer.provideMerge(IntakePolicyLive),
		Layer.provideMerge(HostSuspendMonitorLive),
		Layer.provideMerge(ProductInstructionsLive),
	);
	const graph = AgentGraphOrchestratorLive.pipe(
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(infrastructure),
		Layer.provideMerge(guidance),
		Layer.provideMerge(ProductInstructionsLive),
	);
	const thread_metadata = ThreadMetadataRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const metadata_refinement =
		options.thread_metadata_refiner === undefined
			? ThreadMetadataRefinementCoordinatorDisabled
			: ThreadMetadataRefinementCoordinatorLive.pipe(
					Layer.provideMerge(
						make_thread_metadata_refinement_worker_layer().pipe(
							Layer.provideMerge(options.thread_metadata_refiner),
							Layer.provideMerge(thread_metadata),
						),
					),
					Layer.provideMerge(thread_metadata),
					Layer.provideMerge(persistence),
					Layer.provideMerge(infrastructure),
				);
	const project_affinity = ThreadProjectAffinityRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const project_catalog = ProjectCatalogLive.pipe(Layer.provideMerge(infrastructure));
	/** Workspace ids are minted Snowflakes; the registry is the durable allocator. */
	const project_identities = ProjectIdentityRegistryLive.pipe(Layer.provideMerge(infrastructure));
	const project_locator = (
		options.project_locator ??
		make_node_project_locator_layer().pipe(Layer.provideMerge(NodeProcessRunnerLive))
	).pipe(Layer.provide(project_identities));
	const native_directory_picker =
		options.native_directory_picker ?? NativeDirectoryPickerUnavailable;
	const project_affinity_coordination =
		options.project_locator === undefined
			? ThreadProjectAffinityCoordinatorDisabled
			: ThreadProjectAffinityCoordinatorLive.pipe(
					Layer.provideMerge(project_locator),
					Layer.provideMerge(project_affinity),
					Layer.provideMerge(persistence),
					Layer.provideMerge(infrastructure),
				);
	const project_directories =
		options.project_directory_service ??
		Layer.unwrap(
			ResolveBackendRuntimeConfiguration().pipe(
				Effect.map((runtime_config) =>
					make_project_directory_service_layer(
						options.project_directory_roots ?? [
							runtime_config.home_directory,
							runtime_config.current_directory,
						],
						runtime_config.home_directory,
					),
				),
				Effect.provide(NodeFileSystem.layer),
				Effect.provide(NodePath.layer),
			),
		).pipe(
			Layer.provideMerge(project_locator),
			Layer.provideMerge(native_directory_picker),
			Layer.provideMerge(NodeFileSystem.layer),
			Layer.provideMerge(NodePath.layer),
			Layer.provideMerge(infrastructure),
		);
	const retention_policy = ThreadRetentionPolicyServiceLive.pipe(Layer.provideMerge(persistence));
	const model_favorites = ModelFavoritesServiceLive.pipe(Layer.provideMerge(persistence));
	const threads = ThreadCommandsLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(thread_metadata),
		Layer.provideMerge(project_affinity),
		Layer.provideMerge(retention_policy),
		Layer.provideMerge(model_favorites),
		Layer.provideMerge(session_defaults),
	);
	const terminal_persistence = TerminalRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const terminal_driver = options.terminal_driver ?? NodePtyTerminalDriverLive;
	const terminals = TerminalSessionServiceLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(terminal_persistence),
		Layer.provideMerge(terminal_driver),
		Layer.provideMerge(infrastructure),
	);
	const workspace_discovery = WorkspaceFileDiscoveryLive.pipe(
		Layer.provideMerge(workspace_filesystems),
	);
	const tool_capabilities = ArtisanBuiltInToolCapabilityStateLive.pipe(
		Layer.provideMerge(workspace_bounded_filesystems),
		Layer.provideMerge(workspace_filesystems),
		Layer.provideMerge(workspace_git_registry),
	);
	const tool_registry = make_artisan_tool_registry_layer().pipe(
		Layer.provideMerge(ArtisanToolApprovalPolicyLive),
		Layer.provideMerge(tool_capabilities),
	);
	const tool_repository = ToolInvocationRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const tool_handlers = ExecuteToolLive.pipe(
		Layer.provideMerge(workspace_discovery),
		Layer.provideMerge(workspace_filesystems),
		Layer.provideMerge(workspace_files),
		Layer.provideMerge(workspace_evidence),
		Layer.provideMerge(git),
		Layer.provideMerge(terminals),
		Layer.provideMerge(persistence),
		Layer.provideMerge(infrastructure),
	);
	const tools = ToolControlPlaneLive.pipe(
		Layer.provideMerge(ArtisanToolApprovalPolicyLive),
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(workspace_discovery),
		Layer.provideMerge(tool_handlers),
		Layer.provideMerge(persistence),
		Layer.provideMerge(infrastructure),
		Layer.provideMerge(tool_registry),
		Layer.provideMerge(tool_repository),
	);
	const preview_repository = PreviewRepositoryLive.pipe(Layer.provide(infrastructure));
	const preview_service = PreviewServiceLive.pipe(Layer.provide(preview_repository));
	const preview_targets = make_preview_target_layer().pipe(
		Layer.provide(
			Layer.mergeAll(
				options.preview_health_probe ?? NodePreviewHealthProbeLive,
				PreviewTargetClockLive,
			),
		),
	);
	const preview_inspection = make_preview_inspection_layer().pipe(
		Layer.provide(
			Layer.mergeAll(
				preview_targets,
				options.preview_inspection_connector ?? PreviewInspectionConnectorUnavailableLive,
			),
		),
	);
	const preview_browser = (
		options.preview_external_browser ?? NodePreviewExternalBrowserLive
	).pipe(Layer.provide(preview_targets));
	const preview_rich_links = options.preview_rich_links ?? make_node_rich_link_metadata_layer();
	const previews = PreviewCoordinatorLive.pipe(
		Layer.provide(
			Layer.mergeAll(
				preview_repository,
				preview_service,
				preview_targets,
				preview_inspection,
				preview_browser,
				preview_rich_links,
			),
		),
	);
	const commands = CommandRouterLive.pipe(
		Layer.provideMerge(threads),
		Layer.provideMerge(orchestration),
		Layer.provideMerge(graph),
		Layer.provideMerge(terminals),
		Layer.provideMerge(runtime_catalog),
	);
	/** Marketplace defaults deliberately deny source access and transport startup. Acquisition is inert. */
	const routine_repository = RoutineRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const routine_inspector =
		options.routine_source_inspector ??
		Layer.succeed(RoutineSourceInspector, {
			Inspect: () => Effect.fail(new RoutineInspectorError({ code: "unsupported" })),
		});
	const routine_installer =
		options.routine_installer ??
		Layer.succeed(RoutineInstaller, {
			Install: () => Effect.fail(new RoutineInstallerError({ code: "install_failed" })),
			Rollback: () => Effect.fail(new RoutineInstallerError({ code: "rollback_failed" })),
		});
	const npx_skills =
		options.npx_skills_adapter ??
		Layer.succeed(NpxSkillsAdapter, {
			Discover: () => Effect.fail(new RoutineInspectorError({ code: "unsupported" })),
		});
	const routines = RoutineServiceLive.pipe(
		Layer.provideMerge(routine_repository),
		Layer.provideMerge(routine_inspector),
		Layer.provideMerge(routine_installer),
		Layer.provideMerge(npx_skills),
		Layer.provideMerge(options.routine_mirror_registry ?? EmptyRoutineMirrorRegistryLive),
	);
	const capability_repository = CapabilityRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const oauth = make_oauth_layer.pipe(
		Layer.provideMerge(options.oauth_adapter ?? EmptyOAuthAdapterLive),
	);
	const capability_transport_registry =
		options.capability_transport_registry ?? EmptyCapabilityTransportRegistryLive;
	const capabilities = CapabilityServiceLive.pipe(
		Layer.provideMerge(capability_repository),
		Layer.provideMerge(capability_transport_registry),
	);
	const capability_oauth = CapabilityOAuthLifecycleLive.pipe(
		Layer.provideMerge(capability_repository),
		Layer.provideMerge(oauth),
	);
	const capability_mirrors = CapabilityMirrorServiceLive.pipe(
		Layer.provideMerge(capability_repository),
		Layer.provideMerge(options.capability_provider_mirror ?? EmptyCapabilityProviderMirrorLive),
		Layer.provideMerge(infrastructure),
	);
	const resource_quiescer =
		options.thread_resource_quiescer ??
		ThreadResourceQuiescerLive.pipe(
			Layer.provideMerge(orchestration),
			Layer.provideMerge(graph),
			Layer.provideMerge(terminals),
			Layer.provideMerge(previews),
		);
	const erasure = ThreadErasureLive.pipe(
		Layer.provideMerge(resource_quiescer),
		Layer.provideMerge(infrastructure),
	);
	const retention = ThreadRetentionLive.pipe(
		Layer.provideMerge(options.retention_clock ?? ThreadRetentionClockLive),
		Layer.provideMerge(options.retention_scheduler ?? ThreadRetentionSchedulerLive),
		Layer.provideMerge(retention_policy),
		Layer.provideMerge(erasure),
	);
	const routing = ProtocolRouterLive.pipe(
		Layer.provideMerge(commands),
		Layer.provideMerge(terminals),
	);
	/**
	 * Derived from durable run and queue state, so it composes against the
	 * shared database and journal notifier rather than any orchestration
	 * service: work someone else's instance queued still holds this host awake.
	 */
	const wake_lock =
		options.wake_lock_backend === undefined
			? SystemWakeLockDisabled
			: MakeSystemWakeLockLayer().pipe(
					Layer.provideMerge(options.wake_lock_backend),
					Layer.provideMerge(infrastructure),
				);
	const surfaces = SurfaceServiceLive.pipe(Layer.provideMerge(infrastructure));

	const protocol_foundation = make_protocol_server_layer(protocol_options).pipe(
		Layer.provideMerge(routing),
		Layer.provideMerge(retention_policy),
		Layer.provideMerge(graph),
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(persistence),
		Layer.provideMerge(erasure),
		Layer.provideMerge(retention),
		Layer.provideMerge(metadata_refinement),
		Layer.provideMerge(project_affinity_coordination),
		Layer.provideMerge(project_directories),
		Layer.provideMerge(project_catalog),
		Layer.provideMerge(runtime_catalog),
		Layer.provideMerge(guidance),
		Layer.provideMerge(git),
		Layer.provideMerge(model_behaviour),
		Layer.provideMerge(harness_config),
		Layer.provideMerge(workspace_files),
		Layer.provideMerge(workspace_changes),
		Layer.provideMerge(workspace_diffs),
	);
	const protocol = protocol_foundation.pipe(
		Layer.provideMerge(surfaces),
		Layer.provideMerge(tools),
		Layer.provideMerge(previews),
		Layer.provideMerge(routines),
		Layer.provideMerge(capabilities),
		Layer.provideMerge(capability_repository),
		Layer.provideMerge(capability_oauth),
		Layer.provideMerge(capability_mirrors),
	);

	/**
	 * Every attached project becomes a workspace over its own directory. This
	 * belongs in the composition rather than in the catalog: the catalog owns
	 * persistence, and the registries own capabilities, so the binding between
	 * them is the composition's job.
	 */
	return Layer.mergeAll(protocol, projection_rebuild, ProjectWorkspaceBindingLive, wake_lock).pipe(
		Layer.provideMerge(options.preview_health_probe ?? NodePreviewHealthProbeLive),
		Layer.provideMerge(workspace_evidence),
		Layer.provideMerge(workspace_authority),
		Layer.provideMerge(workspace_bounded_filesystems),
		Layer.provideMerge(workspace_filesystems),
		Layer.provideMerge(workspace_snapshots),
		Layer.provideMerge(workspace_mutation_payloads),
		Layer.provideMerge(project_catalog),
	);
}

function make_desktop_guidance_registry(options: DesktopBackendOptions) {
	return ResolveBackendRuntimeConfiguration(options.guidance_platform).pipe(
		Effect.map((runtime_config) =>
			make_platform_guidance_provider_registry_layer({
				codex_agents_path: join(runtime_config.codex_home, "AGENTS.md"),
				codex_override_path: join(runtime_config.codex_home, "AGENTS.override.md"),
			}).pipe(Layer.provide(GuidanceFileStoreLive)),
		),
	);
}

/**
 * Declares the harness config documents this installation may edit.
 *
 * Only harnesses listed here are writable. Claude's target is declared even
 * though no key currently maps to it, so adding one is a registry edit rather
 * than a path-resolution change.
 */
function make_desktop_harness_config_registry(options: DesktopBackendOptions) {
	const backups_directory =
		options.harness_config_platform?.backups_directory ??
		join(dirname(options.database_path), "harness-config", "backups");

	return ResolveBackendRuntimeConfiguration(options.harness_config_platform).pipe(
		Effect.map((runtime_config) =>
			MakeHarnessConfigRegistryLayer({
				targets: [
					{
						backups_directory,
						format: "toml",
						harness_id: "codex",
						path: join(runtime_config.codex_home, "config.toml"),
					},
					{
						backups_directory,
						format: "json",
						harness_id: "claude",
						path: join(runtime_config.claude_home, "settings.json"),
					},
				],
			}).pipe(Layer.orDie),
		),
	);
}

function make_desktop_model_behaviour_registry(options: DesktopBackendOptions) {
	const model_behaviour_directory = join(dirname(options.database_path), "model-behaviour");
	const probe = make_codex_model_behaviour_probe_layer({
		...(options.model_behaviour_platform?.codex_command === undefined
			? {}
			: { command: options.model_behaviour_platform.codex_command }),
		cwd: dirname(options.database_path),
	}).pipe(Layer.provide(NodeProcessRunnerLive));

	return ResolveBackendRuntimeConfiguration(options.model_behaviour_platform).pipe(
		Effect.map((runtime_config) =>
			make_desktop_model_behaviour_provider_registry_layer({
				backups_directory:
					options.model_behaviour_platform?.backups_directory ??
					join(model_behaviour_directory, "backups"),
				codex_config_path: join(runtime_config.codex_home, "config.toml"),
			}).pipe(Layer.provideMerge(ModelBehaviourConfigFilesLive), Layer.provideMerge(probe)),
		),
	);
}

/**
 * Engines the desktop production composition may load. Anything else fails
 * closed before composing, so an experimental adapter cannot reach a user
 * installation by simply being passed in.
 */
const production_engine_ids: ReadonlySet<string> = new Set(["codex", "claude"]);

/** Builds the production desktop layer with opinionated platform guidance discovery. */
export function make_desktop_backend_layer(options: DesktopBackendOptions) {
	const production_capability_transports = CapabilityTransportRegistryLive.pipe(
		Layer.provideMerge(EngineProcessStdioMcpDriverLive),
		Layer.provideMerge(EngineProcessFactoryLive),
		Layer.provideMerge(EffectHttpMcpDriverLive),
		Layer.provideMerge(options.secret_store ?? EmptySecretStoreLive),
		Layer.provideMerge(FetchHttpClient.layer),
	);
	return Layer.unwrap(
		Effect.gen(function* () {
			if (
				(options.engines ?? []).some(
					(engine) => !production_engine_ids.has(engine.Descriptor.id),
				)
			) {
				return yield* new DesktopEngineConfigurationError({
					message: "Desktop production accepts only the Codex and Claude engines.",
				});
			}

			const guidance_provider_registry =
				options.guidance_provider_registry ??
				(yield* make_desktop_guidance_registry(options));
			const model_behaviour_provider_registry =
				options.model_behaviour_provider_registry ??
				(yield* make_desktop_model_behaviour_registry(options));
			const harness_config_registry =
				options.harness_config_registry ??
				(yield* make_desktop_harness_config_registry(options));

			return make_backend_layer({
				...options,
				native_directory_picker:
					options.native_directory_picker ??
					make_node_native_directory_picker_layer(process.platform),
				harness_config_registry,
				capability_transport_registry:
					options.capability_transport_registry ?? production_capability_transports,
				routine_source_inspector:
					options.routine_source_inspector ?? make_local_routine_source_inspector_layer(),
				routine_installer:
					options.routine_installer ??
					make_local_routine_installer_layer({
						install_root: join(
							dirname(options.database_path),
							"marketplace",
							"routines",
						),
					}),
				...(options.npx_skills_adapter !== undefined
					? { npx_skills_adapter: options.npx_skills_adapter }
					: options.npx_skills_process === undefined
						? {}
						: {
								npx_skills_adapter: make_npx_skills_process_adapter_layer(
									options.npx_skills_process,
								).pipe(Layer.provide(EngineProcessFactoryLive)),
							}),
				guidance_provider_registry,
				model_behaviour_provider_registry,
				project_locator:
					options.project_locator ??
					make_node_project_locator_layer().pipe(Layer.provide(NodeProcessRunnerLive)),
				thread_metadata_refiner:
					options.thread_metadata_refiner ?? ThreadMetadataRefinerLive,
				wake_lock_backend:
					options.wake_lock_backend ??
					make_platform_wake_lock_backend_layer(process.platform),
			});
		}).pipe(Effect.provide(NodeFileSystem.layer), Effect.provide(NodePath.layer)),
	);
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}

/** Builds the production desktop runtime rather than the provider-neutral test core. */
export function make_desktop_backend_runtime(options: DesktopBackendOptions) {
	return ManagedRuntime.make(make_desktop_backend_layer(options));
}
