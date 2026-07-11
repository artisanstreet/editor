import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { Layer, ManagedRuntime } from "effect";

import { type Engine, make_engine_registry_layer } from "@artisan/engines";
import type { GlobalGuidanceProvider } from "@artisan/protocol";

import { AgentGraphOrchestratorLive } from "../orchestration/agent-graph-orchestrator";
import { AgentGraphRepositoryLive } from "../orchestration/agent-graph-repository";
import { AgentOrchestratorLive } from "../orchestration/agent-orchestrator";
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

export interface BackendOptions {
	readonly database_path: string;
	readonly engines?: ReadonlyArray<Engine>;
	readonly guidance?: Partial<GlobalGuidanceServiceOptions>;
	readonly guidance_provider_registry?: Layer.Layer<GuidanceProviderRegistry>;
	readonly migrations_path: string;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
	readonly retention_clock?: Layer.Layer<ThreadRetentionClock>;
	readonly retention_scheduler?: Layer.Layer<ThreadRetentionScheduler>;
	readonly runtime_metadata?: Layer.Layer<RuntimeMetadata>;
	readonly terminal_driver?: Layer.Layer<TerminalDriver>;
	readonly thread_resource_quiescer?: Layer.Layer<ThreadResourceQuiescer>;
}

/** Configures platform-native provider discovery for the production desktop composition. */
export interface DesktopGuidanceOptions {
	readonly claude_config_directory?: string;
	readonly codex_home?: string;
	readonly home_directory?: string;
	readonly providers?: ReadonlyArray<GlobalGuidanceProvider>;
}

/** Extends the portable backend options with desktop provider-path discovery. */
export interface DesktopBackendOptions extends BackendOptions {
	readonly guidance_platform?: DesktopGuidanceOptions;
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
	const retention_policy = ThreadRetentionPolicyServiceLive.pipe(Layer.provideMerge(persistence));
	const threads = ThreadCommandsLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(thread_metadata),
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

	return make_protocol_server_layer(protocol_options).pipe(
		Layer.provideMerge(routing),
		Layer.provideMerge(retention_policy),
		Layer.provideMerge(graph),
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(persistence),
		Layer.provideMerge(erasure),
		Layer.provideMerge(retention),
		Layer.provideMerge(guidance),
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

/** Builds the production desktop layer with opinionated platform guidance discovery. */
export function make_desktop_backend_layer(options: DesktopBackendOptions) {
	return make_backend_layer({
		...options,
		guidance_provider_registry:
			options.guidance_provider_registry ?? make_desktop_guidance_registry(options),
	});
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}

/** Builds the production desktop runtime rather than the provider-neutral test core. */
export function make_desktop_backend_runtime(options: DesktopBackendOptions) {
	return ManagedRuntime.make(make_desktop_backend_layer(options));
}
