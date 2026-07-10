import { Layer, ManagedRuntime } from "effect";

import { make_database_layer } from "../persistence/database";
import { JournalNotifierLive } from "../persistence/journal-notifier";
import { JournalStoreLive } from "../persistence/journal-store";
import { ThreadReadModelLive } from "../persistence/thread-read-model";
import { CommandRouterLive } from "../protocol/command-router";
import {
	DefaultProtocolConnectionOptions,
	type ProtocolConnectionOptions,
} from "../protocol/protocol-connection";
import { ProtocolRouterLive } from "../protocol/protocol-router";
import { make_protocol_server_layer } from "../protocol/protocol-server";
import { ThreadCommandsLive } from "../threads/thread-commands";
import { RuntimeMetadataLive } from "./runtime-metadata";

export interface BackendOptions {
	readonly database_path: string;
	readonly migrations_path: string;
	readonly protocol?: Partial<ProtocolConnectionOptions>;
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
	const persistence = Layer.mergeAll(JournalStoreLive, ThreadReadModelLive).pipe(
		Layer.provideMerge(infrastructure),
	);
	const threads = ThreadCommandsLive.pipe(Layer.provideMerge(persistence));
	const commands = CommandRouterLive.pipe(Layer.provideMerge(threads));
	const routing = ProtocolRouterLive.pipe(Layer.provideMerge(commands));

	return make_protocol_server_layer(protocol_options).pipe(Layer.provideMerge(routing));
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}
