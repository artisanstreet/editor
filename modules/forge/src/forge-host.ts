import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Exit, FileSystem, Layer, Path, Scope } from "effect";

import { make_desktop_backend_layer, RichLinkAssetStoreLive } from "@artisan/backend";
import { CodexEngine, CodexProcessFactoryLive, make_codex_engine_layer } from "@artisan/engines";
import {
	make_backend_message_port_transport_server_layer,
	MessagePortTransportServer,
} from "@artisan/transport/server";
import { MakeWebSocketServerSession } from "@artisan/transport/websocket/server";

import { ForgeControlAuthority, make_forge_control_authority_layer } from "./control-authority";
import type { ForgeConfig } from "./config";
import { AcquireForgeDatabaseLease } from "./database-lease";
import { start_forge_http } from "./http-host";
import type { ForgeTransportBindingService } from "./transport-binding";
import { BindForgeWebSocket } from "./websocket-binding";

const ForgeWebSocketBinding: ForgeTransportBindingService = {
	Bind: BindForgeWebSocket,
};

export interface ForgeHandle {
	readonly Close: Effect.Effect<void>;
	readonly endpoint: URL;
	readonly RequestShutdown: Effect.Effect<void>;
	readonly ShutdownRequested: Effect.Effect<void>;
}

const MakeForgeHost = (config: ForgeConfig, transport_binding: ForgeTransportBindingService) =>
	Effect.gen(function* () {
		const lease = yield* AcquireForgeDatabaseLease(config.database_path);
		void lease;
		const protocol_server = yield* MessagePortTransportServer;
		const authority = yield* ForgeControlAuthority;
		const http = yield* Effect.acquireRelease(start_forge_http(config, authority), (server) =>
			server.Close.pipe(Effect.ignore),
		);
		yield* Effect.acquireRelease(
			transport_binding.Bind({
				authority,
				config,
				http,
				ServeWebSocket: (endpoint, peer) =>
					Effect.scoped(
						MakeWebSocketServerSession(endpoint, peer, protocol_server.Serve, {
							require_loopback: true,
						}),
					),
			}),
			(transport) => transport.Close.pipe(Effect.ignore),
		);

		return {
			endpoint: http.endpoint,
			ReleaseLease: lease.Release,
			RequestShutdown: authority.RequestShutdown,
			ShutdownRequested: authority.ShutdownRequested,
		};
	}).pipe(Effect.onError(() => Effect.logError("Artisan Forge host acquisition failed")));

const MakeForgeRuntime = (config: ForgeConfig) => {
	const engine_layer = make_codex_engine_layer().pipe(Layer.provide(CodexProcessFactoryLive));
	const backend_layer = Layer.unwrap(
		Effect.gen(function* () {
			const codex_engine = yield* CodexEngine;
			return make_desktop_backend_layer({
				database_path: config.database_path,
				engines: [codex_engine],
				migrations_path: config.migrations_path,
				project_directory_roots: config.project_directory_roots,
			});
		}),
	).pipe(Layer.provide(engine_layer));
	const transport_layer = make_backend_message_port_transport_server_layer().pipe(
		Layer.provideMerge(backend_layer),
		Layer.provideMerge(RichLinkAssetStoreLive),
	);

	return Layer.mergeAll(engine_layer, transport_layer, make_forge_control_authority_layer());
};

/**
 * Starts the standalone, host-neutral Forge process. Electron may launch it,
 * but Forge owns its state and can run independently on a VM.
 */
export const StartForge = (
	config: ForgeConfig,
	transport_binding: ForgeTransportBindingService = ForgeWebSocketBinding,
) =>
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const path = yield* Path.Path;
		yield* file_system.makeDirectory(path.dirname(config.database_path), { recursive: true });
		const host_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
			Scope.close(scope, Exit.void),
		);
		const runtime = yield* Layer.build(MakeForgeRuntime(config)).pipe(
			Effect.provideService(Scope.Scope, host_scope),
		);
		const host = yield* MakeForgeHost(config, transport_binding).pipe(
			Effect.provide(runtime),
			Effect.provideService(Scope.Scope, host_scope),
		);
		return {
			endpoint: host.endpoint,
			RequestShutdown: host.RequestShutdown,
			ShutdownRequested: host.ShutdownRequested,
			Close: host.ReleaseLease.pipe(
				Effect.ignore,
				Effect.ensuring(Scope.close(host_scope, Exit.void)),
			),
		} satisfies ForgeHandle;
	}).pipe(Effect.provide(NodeFileSystem.layer), Effect.provide(NodePath.layer));
