import { dirname, join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Clock, Effect, Exit, FileSystem, Layer, Path, Scope } from "effect";

import {
	make_desktop_backend_layer,
	RichLinkAssetStoreLive,
	ThreadMetadataRefinementCoordinator,
} from "@artisan/backend";
import {
	ClaudeEngine,
	CodexEngine,
	CodexProcessFactoryLive,
	make_claude_engine_layer,
	make_codex_engine_layer,
} from "@artisan/engines";
import {
	make_backend_message_port_transport_server_layer,
	MessagePortTransportServer,
} from "@artisan/transport/server";
import { MakeWebSocketServerSession } from "@artisan/transport/websocket/server";

import { ForgeControlAuthority, make_forge_control_authority_layer } from "./control-authority";
import type { ForgeConfig } from "./config";
import { AcquireForgeDatabaseLease } from "./database-lease";
import { start_forge_http } from "./http-host";
import { InstanceCardPath } from "./instance-registry";
import { RemoveForgeState, WriteForgeState } from "./state";
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
		const metadata_refinement = yield* ThreadMetadataRefinementCoordinator;
		/** Repair missed automatic metadata before the first browser can observe stale titles. */
		yield* metadata_refinement.WaitForIdle;
		const authority = yield* ForgeControlAuthority;
		const http = yield* Effect.acquireRelease(start_forge_http(config, authority), (server) =>
			server.Close.pipe(Effect.ignore),
		);
		if (config.instance_registry_root !== undefined) {
			/**
			 * Announcement lives with the host, not the launcher, so every start
			 * path — detached entry, foreground CLI, embedded composition — is
			 * discoverable. It is best-effort: a Forge that cannot write its card
			 * still serves its own origin, it just stays invisible to the picker.
			 */
			const card_path = InstanceCardPath(config.instance_registry_root, config.instance_id);
			const now = yield* Clock.currentTimeMillis;
			yield* Effect.acquireRelease(
				WriteForgeState(card_path, {
					endpoint: http.endpoint.toString(),
					instance_id: config.instance_id,
					pid: process.pid,
					started_at: new Date(now).toISOString(),
					version: 1,
				}).pipe(
					Effect.catch((failure) =>
						Effect.logWarning(
							`Forge instance card was not announced: ${String(failure.cause)}`,
						),
					),
				),
				() => RemoveForgeState(card_path, config.instance_id).pipe(Effect.ignore),
			);
		}
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
	const engine_layer = Layer.mergeAll(make_codex_engine_layer(), make_claude_engine_layer()).pipe(
		Layer.provide(CodexProcessFactoryLive),
	);
	const backend_layer = Layer.unwrap(
		Effect.gen(function* () {
			const codex_engine = yield* CodexEngine;
			const claude_engine = yield* ClaudeEngine;
			return make_desktop_backend_layer({
				database_path: config.database_path,
				engines: [codex_engine, claude_engine],
				migrations_path: config.migrations_path,
			});
		}),
	).pipe(Layer.provide(engine_layer));
	const transport_layer = make_backend_message_port_transport_server_layer().pipe(
		Layer.provideMerge(backend_layer),
		Layer.provideMerge(RichLinkAssetStoreLive),
	);

	return Layer.mergeAll(
		engine_layer,
		transport_layer,
		/**
		 * Session digests persist beside the database so a Forge restart —
		 * notably an update — does not log out every paired browser.
		 */
		make_forge_control_authority_layer({
			session_store_path: join(dirname(config.database_path), "forge-sessions.json"),
		}),
	);
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
