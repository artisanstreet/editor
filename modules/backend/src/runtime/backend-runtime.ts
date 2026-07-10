import { Layer, ManagedRuntime } from "effect";

import { make_database_layer } from "../persistence/database";
import { JournalStoreLive } from "../persistence/journal-store";
import { CommandRouterLive } from "../protocol/command-router";
import { ProtocolRouterLive } from "../protocol/protocol-router";
import { ThreadCommandsLive } from "../threads/thread-commands";
import { RuntimeMetadataLive } from "./runtime-metadata";

export interface BackendOptions {
	readonly database_path: string;
	readonly migrations_path: string;
}

export function make_backend_layer(options: BackendOptions) {
	const infrastructure = Layer.mergeAll(make_database_layer(options), RuntimeMetadataLive);

	const journal = JournalStoreLive.pipe(Layer.provideMerge(infrastructure));
	const threads = ThreadCommandsLive.pipe(Layer.provideMerge(journal));
	const commands = CommandRouterLive.pipe(Layer.provideMerge(threads));

	return ProtocolRouterLive.pipe(Layer.provide(commands));
}

export function make_backend_runtime(options: BackendOptions) {
	return ManagedRuntime.make(make_backend_layer(options));
}
