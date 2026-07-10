import { Layer, ManagedRuntime } from "effect";

import { type Engine, make_engine_registry_layer } from "@artisan/engines";

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
import { NodePtyTerminalDriverLive } from "../terminal/node-pty-terminal-driver";
import { TerminalDriver } from "../terminal/terminal-driver";
import { TerminalRepositoryLive } from "../terminal/terminal-repository";
import { TerminalSessionServiceLive } from "../terminal/terminal-sessions";
import { RuntimeMetadataLive } from "./runtime-metadata";

export interface BackendOptions {
	readonly database_path: string;
	readonly engines?: ReadonlyArray<Engine>;
	readonly migrations_path: string;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
	readonly terminal_driver?: Layer.Layer<TerminalDriver>;
}

export function make_backend_layer(options: BackendOptions) {
	const protocol_options: ProtocolConnectionOptions = {
		...DefaultProtocolConnectionOptions,
		...options.protocol,
	};
	const infrastructure = Layer.mergeAll(
		make_database_layer(options),
		RuntimeMetadataLive,
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
	const threads = ThreadCommandsLive.pipe(Layer.provideMerge(persistence));
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
		Layer.provideMerge(terminals),
	);
	const routing = ProtocolRouterLive.pipe(
		Layer.provideMerge(commands),
		Layer.provideMerge(terminals),
	);

	return make_protocol_server_layer(protocol_options).pipe(
		Layer.provideMerge(routing),
		Layer.provideMerge(persistence),
	);
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}
