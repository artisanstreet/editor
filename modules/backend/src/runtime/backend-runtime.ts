import { Layer, ManagedRuntime } from "effect";

import { type Engine, make_engine_registry_layer } from "@artisan/engines";

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

export interface BackendOptions {
	readonly database_path: string;
	readonly engines?: ReadonlyArray<Engine>;
	readonly migrations_path: string;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
	readonly retention_clock?: Layer.Layer<ThreadRetentionClock>;
	readonly retention_scheduler?: Layer.Layer<ThreadRetentionScheduler>;
	readonly runtime_metadata?: Layer.Layer<RuntimeMetadata>;
	readonly terminal_driver?: Layer.Layer<TerminalDriver>;
	readonly thread_resource_quiescer?: Layer.Layer<ThreadResourceQuiescer>;
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
	const orchestration = AgentOrchestratorLive.pipe(
		Layer.provideMerge(persistence),
		Layer.provideMerge(engine_registry),
	);
	const graph_persistence = AgentGraphRepositoryLive.pipe(Layer.provideMerge(infrastructure));
	const graph = AgentGraphOrchestratorLive.pipe(
		Layer.provideMerge(graph_persistence),
		Layer.provideMerge(engine_registry),
		Layer.provideMerge(infrastructure),
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
	);
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}
