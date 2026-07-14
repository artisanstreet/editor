import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { NodeCrypto, NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, ManagedRuntime } from "effect";

import { type Engine, make_engine_registry_layer } from "@artisan/engines";
import type { GlobalGuidanceProvider } from "@artisan/protocol";

import { AgentGraphOrchestratorLive } from "../orchestration/agent-graph-orchestrator";
import { AgentGraphRepositoryLive } from "../orchestration/agent-graph-repository";
import { AgentOrchestratorLive } from "../orchestration/agent-orchestrator";
import {
	make_node_workspace_filesystem_registry_layer,
	WorkspaceFilesystemRegistrationError,
	WorkspaceFilesystemRegistry,
} from "../filesystem/workspace-filesystem-registry";
import {
	EmptyWorkspaceBoundedRegularFileStoreRegistryLive,
	WorkspaceBoundedRegularFileStoreRegistrationError,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../filesystem/workspace-bounded-regular-file-store-registry";
import { NativeBoundedRegularFileStoreInitializationError } from "../filesystem/native-bounded-regular-file-store";
import { NodeProcessRunnerLive } from "../git/node-process-runner";
import {
	EmptyWorkspaceGitRegistryLive,
	WorkspaceGitRegistrationError,
	WorkspaceGitRegistry,
} from "../git/workspace-git-registry";
import { WorkspaceGitObserverLive } from "../git/workspace-git-observer";
import { WorkspaceGitSessionRepositoryLive } from "../git/workspace-git-session-repository";
import { WorkspaceGitSessionServiceLive } from "../git/workspace-git-session-service";
import { WorkspaceGitCheckoutRepositoryLive } from "../git/workspace-git-checkout-repository";
import { WorkspaceGitCheckoutCoordinatorLive } from "../git/workspace-git-checkout-coordinator";
import { make_workspace_git_execution_gate_layer } from "../git/workspace-git-execution-gate";
import { WorkspaceGitMutationRepositoryLive } from "../git/workspace-git-mutation-repository";
import { WorkspaceGitMutationCoordinatorLive } from "../git/workspace-git-mutation-coordinator";
import { GitProvider } from "../git-provider/git-provider";
import {
	EmptyGitProviderRegistryLive,
	GitProviderRegistry,
	GitProviderRegistryError,
	make_git_provider_registry_layer,
} from "../git-provider/git-provider-registry";
import { make_github_cli_layer } from "../git-provider/github/github-cli";
import { make_node_github_cli_executable_layer } from "../git-provider/github/github-cli-executable";
import { make_github_provider_layer } from "../git-provider/github/github-provider";
import { make_database_layer } from "../persistence/database";
import { JournalNotifierLive } from "../persistence/journal-notifier";
import { JournalStoreLive } from "../persistence/journal-store";
import { OrchestrationRepositoryLive } from "../persistence/orchestration-repository";
import { ThreadReadModelLive } from "../persistence/thread-read-model";
import { CommandRouterLive } from "../protocol/command-router";
import {
	DefaultProtocolConnectionOptions,
	type ProtocolConnectionOptions,
} from "../protocol/protocol-connection";
import { ProtocolRouterLive } from "../protocol/protocol-router";
import { make_protocol_server_layer } from "../protocol/protocol-server";
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
	ThreadProjectAffinityCoordinatorDisabled,
	ThreadProjectAffinityCoordinatorLive,
} from "../threads/thread-project-affinity-coordinator";
import { ThreadProjectAffinityRepositoryLive } from "../threads/thread-project-affinity-repository";
import {
	ThreadResourceQuiescer,
	ThreadResourceQuiescerLive,
} from "../threads/thread-resource-quiescer";
import { ThreadRetentionPolicyServiceLive } from "../threads/thread-retention-policy";
import {
	ThreadRetentionClock,
	ThreadRetentionClockLive,
	ThreadRetentionLive,
	ThreadRetentionScheduler,
	ThreadRetentionSchedulerLive,
} from "../threads/thread-retention";
import { NodePtyTerminalDriverLive } from "../terminal/node-pty-terminal-driver";
import { TerminalDriver } from "../terminal/terminal-driver";
import { TerminalRepositoryLive } from "../terminal/terminal-repository";
import { TerminalSessionServiceLive } from "../terminal/terminal-sessions";
import { RuntimeMetadata, RuntimeMetadataLive } from "./runtime-metadata";
import { GlobalGuidanceRepositoryLive } from "../guidance/guidance-repository";
import {
	make_global_guidance_service_layer,
	type GlobalGuidanceServiceOptions,
} from "../guidance/guidance-service";
import { GuidanceFileStoreLive } from "../guidance/file-store";
import {
	EmptyGuidanceProviderRegistryLive,
	GuidanceProviderRegistry,
	make_platform_guidance_provider_registry_layer,
} from "../guidance/provider-mirrors";
import { make_codex_model_behaviour_probe_layer } from "../model-behaviour/codex-probe";
import { ModelBehaviourConfigFilesLive } from "../model-behaviour/model-behaviour-config-files";
import {
	EmptyModelBehaviourProviderRegistryLive,
	make_desktop_model_behaviour_provider_registry_layer,
	ModelBehaviourProviderRegistry,
} from "../model-behaviour/model-behaviour-provider";
import { ModelBehaviourRepositoryLive } from "../model-behaviour/model-behaviour-repository";
import { ModelBehaviourRegistryError } from "../model-behaviour/model-behaviour-registry";
import { ModelBehaviourServiceLive } from "../model-behaviour/model-behaviour-service";
import { WorkspaceChangeRepositoryLive } from "../workspace/workspace-change-repository";
import { WorkspaceEvidenceRecorderLive } from "../workspace/workspace-evidence-recorder";
import { WorkspaceFileServiceLive } from "../workspace/workspace-file-service";
import { WorkspaceMutationAuthorityLive } from "../workspace/workspace-mutation-authority";
import { WorkspaceSnapshotStoreLive } from "../workspace/workspace-snapshot-store";
import { WorkspaceMutationPayloadStoreLive } from "../workspace/workspace-mutation-payload-store";
import { WorkspaceChangeDiffServiceLive } from "../workspace/workspace-change-diff-service";
import { WorkspaceReplaceApprovalRepositoryLive } from "../workspace/workspace-replace-approval-repository";
import { WorkspaceReplaceApprovalCoordinatorLive } from "../workspace/workspace-replace-approval-coordinator";

export interface BackendOptions {
	readonly database_path: string;
	readonly engines?: ReadonlyArray<Engine>;
	readonly git_provider_registry?: Layer.Layer<GitProviderRegistry, GitProviderRegistryError>;
	readonly guidance?: Partial<GlobalGuidanceServiceOptions>;
	readonly guidance_provider_registry?: Layer.Layer<GuidanceProviderRegistry>;
	readonly migrations_path: string;
	readonly model_behaviour_provider_registry?: Layer.Layer<
		ModelBehaviourProviderRegistry,
		ModelBehaviourRegistryError
	>;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
	readonly project_locator?: Layer.Layer<ProjectLocator>;
	readonly retention_clock?: Layer.Layer<ThreadRetentionClock>;
	readonly retention_scheduler?: Layer.Layer<ThreadRetentionScheduler>;
	readonly runtime_metadata?: Layer.Layer<RuntimeMetadata>;
	readonly terminal_driver?: Layer.Layer<TerminalDriver>;
	readonly thread_metadata_refiner?: Layer.Layer<ThreadMetadataRefiner>;
	readonly thread_resource_quiescer?: Layer.Layer<ThreadResourceQuiescer>;
	readonly workspace_filesystem_registry?: Layer.Layer<
		WorkspaceFilesystemRegistry,
		WorkspaceFilesystemRegistrationError
	>;
	readonly workspace_git_registry?: Layer.Layer<
		WorkspaceGitRegistry,
		WorkspaceGitRegistrationError
	>;
	readonly workspace_bounded_regular_file_store_registry?: Layer.Layer<
		WorkspaceBoundedRegularFileStoreRegistry,
		| NativeBoundedRegularFileStoreInitializationError
		| WorkspaceBoundedRegularFileStoreRegistrationError
	>;
}

/** Configures platform-native provider discovery for the production desktop composition. */
export interface DesktopGuidanceOptions {
	readonly claude_config_directory?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
	readonly providers?: ReadonlyArray<GlobalGuidanceProvider>;
}

/** Configures provider-native Model Behaviour discovery for desktop composition. */
export interface DesktopModelBehaviourOptions {
	readonly backups_directory?: string;
	readonly codex_command?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
}

/** Configures optional GitHub CLI discovery for the production desktop composition. */
export interface DesktopGitProviderOptions {
	readonly command?: string;
	readonly hosts?: ReadonlyArray<string>;
	readonly probe_timeout_ms?: number;
	readonly request_timeout_ms?: number;
}

/** Extends the portable backend options with desktop provider-path discovery. */
export interface DesktopBackendOptions extends BackendOptions {
	readonly git_provider_platform?: DesktopGitProviderOptions;
	readonly guidance_platform?: DesktopGuidanceOptions;
	readonly model_behaviour_platform?: DesktopModelBehaviourOptions;
}

export function make_backend_layer(options: BackendOptions) {
	const protocol_options: ProtocolConnectionOptions = {
		...DefaultProtocolConnectionOptions,
		...options.protocol,
	};
	const infrastructure = Layer.mergeAll(
		make_database_layer(options),
		options.runtime_metadata ?? RuntimeMetadataLive,
		JournalNotifierLive,
	);
	const persistence = Layer.mergeAll(
		JournalStoreLive,
		OrchestrationRepositoryLive,
		ThreadReadModelLive,
	).pipe(Layer.provideMerge(infrastructure));
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
	const workspace_approvals = WorkspaceReplaceApprovalRepositoryLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(infrastructure),
	);
	const workspace_filesystems =
		options.workspace_filesystem_registry ?? make_node_workspace_filesystem_registry_layer([]);
	const workspace_bounded_filesystems =
		options.workspace_bounded_regular_file_store_registry ??
		EmptyWorkspaceBoundedRegularFileStoreRegistryLive;
	const workspace_git_registry = options.workspace_git_registry ?? EmptyWorkspaceGitRegistryLive;
	const git_provider_registry = options.git_provider_registry ?? EmptyGitProviderRegistryLive;
	const workspace_git_observer = WorkspaceGitObserverLive.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(workspace_git_registry),
		Layer.provideMerge(infrastructure),
	);
	const workspace_git_sessions_repository = WorkspaceGitSessionRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const workspace_git_sessions = WorkspaceGitSessionServiceLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(workspace_evidence),
		Layer.provideMerge(workspace_git_observer),
		Layer.provideMerge(workspace_git_sessions_repository),
	);
	const workspace_git_checkouts_repository = WorkspaceGitCheckoutRepositoryLive.pipe(
		Layer.provideMerge(infrastructure),
	);
	const workspace_git_checkouts = WorkspaceGitCheckoutCoordinatorLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(workspace_git_observer),
		Layer.provideMerge(workspace_git_registry),
		Layer.provideMerge(workspace_git_sessions),
		Layer.provideMerge(workspace_git_checkouts_repository),
		Layer.provideMerge(infrastructure),
	);
	const workspace_git_mutations_repository = WorkspaceGitMutationRepositoryLive.pipe(
		Layer.provideMerge(
			make_workspace_git_execution_gate_layer({ database_path: options.database_path }),
		),
		Layer.provideMerge(infrastructure),
	);
	const workspace_git_mutations = WorkspaceGitMutationCoordinatorLive.pipe(
		Layer.provideMerge(NodeCrypto.layer),
		Layer.provideMerge(workspace_git_registry),
		Layer.provideMerge(workspace_git_sessions),
		Layer.provideMerge(workspace_git_mutations_repository),
		Layer.provideMerge(infrastructure),
	);
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
				workspace_approvals,
				workspace_bounded_filesystems,
				workspace_changes,
				workspace_evidence,
				workspace_diffs,
				workspace_mutation_payloads,
				workspace_snapshots,
			),
		),
	);
	const workspace_approval_coordination = WorkspaceReplaceApprovalCoordinatorLive.pipe(
		Layer.provideMerge(workspace_approvals),
		Layer.provideMerge(workspace_files),
	);
	const engine_registry = make_engine_registry_layer(options.engines ?? []);
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
	const orchestration = AgentOrchestratorLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(guidance),
	);
	const graph_persistence = AgentGraphRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const graph = AgentGraphOrchestratorLive.pipe(
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(infrastructure),
		Layer.provideMerge(guidance),
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
	const project_affinity_coordination =
		options.project_locator === undefined
			? ThreadProjectAffinityCoordinatorDisabled
			: ThreadProjectAffinityCoordinatorLive.pipe(
					Layer.provideMerge(options.project_locator),
					Layer.provideMerge(project_affinity),
					Layer.provideMerge(persistence),
					Layer.provideMerge(infrastructure),
				);
	const retention_policy = ThreadRetentionPolicyServiceLive.pipe(Layer.provideMerge(persistence));
	const threads = ThreadCommandsLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(thread_metadata),
		Layer.provideMerge(project_affinity),
		Layer.provideMerge(retention_policy),
	);
	const terminal_persistence = TerminalRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const terminal_driver = options.terminal_driver ?? NodePtyTerminalDriverLive;
	const terminals = TerminalSessionServiceLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(terminal_persistence),
		Layer.provideMerge(terminal_driver),
		Layer.provideMerge(infrastructure),
	);
	const commands = CommandRouterLive.pipe(
		Layer.provideMerge(threads),
		Layer.provideMerge(orchestration),
		Layer.provideMerge(graph),
		Layer.provideMerge(terminals),
	);
	const resource_quiescer =
		options.thread_resource_quiescer ??
		ThreadResourceQuiescerLive.pipe(
			Layer.provideMerge(orchestration),
			Layer.provideMerge(graph),
			Layer.provideMerge(terminals),
			Layer.provideMerge(workspace_approval_coordination),
			Layer.provideMerge(workspace_git_checkouts),
			Layer.provideMerge(workspace_git_mutations),
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
	const workspace = Layer.mergeAll(
		workspace_files,
		workspace_approval_coordination,
		workspace_approvals,
		workspace_changes,
		workspace_diffs,
		workspace_evidence,
		workspace_authority,
		workspace_bounded_filesystems,
		workspace_filesystems,
		workspace_snapshots,
		workspace_mutation_payloads,
		workspace_git_checkouts,
		workspace_git_checkouts_repository,
		workspace_git_mutations,
		workspace_git_mutations_repository,
		workspace_git_observer,
		workspace_git_registry,
		workspace_git_sessions,
		workspace_git_sessions_repository,
	);

	return make_protocol_server_layer(protocol_options).pipe(
		Layer.provideMerge(routing),
		Layer.provideMerge(retention_policy),
		Layer.provideMerge(graph),
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(persistence),
		Layer.provideMerge(erasure),
		Layer.provideMerge(retention),
		Layer.provideMerge(metadata_refinement),
		Layer.provideMerge(project_affinity_coordination),
		Layer.provideMerge(guidance),
		Layer.provideMerge(model_behaviour),
		Layer.provideMerge(git_provider_registry),
		Layer.provideMerge(workspace),
	);
}

function is_guidance_provider(value: string): value is GlobalGuidanceProvider {
	return value === "claude" || value === "codex";
}

function make_desktop_guidance_registry(options: DesktopBackendOptions) {
	const configured_home_directory = options.guidance_platform?.home_directory;
	const home_directory = configured_home_directory ?? homedir();
	const codex_home =
		options.guidance_platform?.codex_home ??
		(configured_home_directory === undefined ? process.env.CODEX_HOME : undefined) ??
		join(home_directory, ".codex");
	const claude_config_directory =
		options.guidance_platform?.claude_config_directory ??
		(configured_home_directory === undefined ? process.env.CLAUDE_CONFIG_DIR : undefined) ??
		join(home_directory, ".claude");
	const providers = [
		...new Set(
			options.guidance_platform?.providers ??
				(options.engines ?? [])
					.map((engine) => engine.Descriptor.id)
					.filter(is_guidance_provider),
		),
	];

	return make_platform_guidance_provider_registry_layer({
		claude_path: join(claude_config_directory, "CLAUDE.md"),
		codex_agents_path: join(codex_home, "AGENTS.md"),
		codex_override_path: join(codex_home, "AGENTS.override.md"),
		providers,
	}).pipe(Layer.provide(GuidanceFileStoreLive));
}

function make_desktop_model_behaviour_registry(options: DesktopBackendOptions) {
	const configured_home_directory = options.model_behaviour_platform?.home_directory;
	const home_directory = configured_home_directory ?? homedir();
	const codex_home =
		options.model_behaviour_platform?.codex_home ??
		(configured_home_directory === undefined ? process.env.CODEX_HOME : undefined) ??
		join(home_directory, ".codex");
	const model_behaviour_directory = join(dirname(options.database_path), "model-behaviour");
	const probe = make_codex_model_behaviour_probe_layer({
		...(options.model_behaviour_platform?.codex_command === undefined
			? {}
			: { command: options.model_behaviour_platform.codex_command }),
		cwd: dirname(options.database_path),
	}).pipe(Layer.provide(NodeProcessRunnerLive));

	return make_desktop_model_behaviour_provider_registry_layer({
		backups_directory:
			options.model_behaviour_platform?.backups_directory ??
			join(model_behaviour_directory, "backups"),
		codex_config_path: join(codex_home, "config.toml"),
	}).pipe(Layer.provideMerge(ModelBehaviourConfigFilesLive), Layer.provideMerge(probe));
}

function make_desktop_git_provider_registry(options: DesktopBackendOptions) {
	const platform = options.git_provider_platform ?? {};
	const cwd = dirname(options.database_path);
	const static_hosts = ["github.com", ...(platform.hosts ?? [])];
	const executable = make_node_github_cli_executable_layer({
		...(platform.command === undefined ? {} : { command: platform.command }),
		cwd,
	});
	const cli = make_github_cli_layer({
		...(platform.command === undefined ? {} : { command: platform.command }),
		cwd,
		...(platform.probe_timeout_ms === undefined
			? {}
			: { probe_timeout_ms: platform.probe_timeout_ms }),
		...(platform.request_timeout_ms === undefined
			? {}
			: { request_timeout_ms: platform.request_timeout_ms }),
	}).pipe(Layer.provideMerge(NodeProcessRunnerLive), Layer.provideMerge(executable));
	const github = make_github_provider_layer(
		platform.hosts === undefined ? {} : { hosts: platform.hosts },
	).pipe(Layer.provide(cli));
	const BuildRegistry = Effect.gen(function* () {
		const provider = yield* GitProvider;

		return yield* GitProviderRegistry.pipe(
			Effect.provide(make_git_provider_registry_layer([{ hosts: static_hosts, provider }])),
		);
	}).pipe(
		Effect.provide(github),
		Effect.mapError((cause) =>
			cause instanceof GitProviderRegistryError
				? cause
				: new GitProviderRegistryError({ reason: "invalid_provider" }),
		),
	);

	return Layer.effect(GitProviderRegistry, BuildRegistry);
}

/** Builds the production desktop layer with opinionated platform guidance discovery. */
export function make_desktop_backend_layer(options: DesktopBackendOptions) {
	return make_backend_layer({
		...options,
		git_provider_registry:
			options.git_provider_registry ?? make_desktop_git_provider_registry(options),
		guidance_provider_registry:
			options.guidance_provider_registry ?? make_desktop_guidance_registry(options),
		model_behaviour_provider_registry:
			options.model_behaviour_provider_registry ??
			make_desktop_model_behaviour_registry(options),
		project_locator:
			options.project_locator ??
			make_node_project_locator_layer().pipe(Layer.provide(NodeProcessRunnerLive)),
		thread_metadata_refiner: options.thread_metadata_refiner ?? ThreadMetadataRefinerLive,
	});
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}

/** Builds the production desktop runtime rather than the provider-neutral test core. */
export function make_desktop_backend_runtime(options: DesktopBackendOptions) {
	return ManagedRuntime.make(make_desktop_backend_layer(options));
}
