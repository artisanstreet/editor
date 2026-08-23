import { dirname, join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Clock, Duration, Effect, Exit, FileSystem, Layer, Path, Scope } from "effect";

import {
  AgentGraphOrchestrator,
  AgentOrchestrator,
  CodexModelBehaviourExecutableUnavailable,
  make_codex_model_behaviour_executable_layer,
  make_desktop_backend_layer,
  ProductTelemetry,
  RichLinkAssetStoreLive,
  TelemetryPreferencesControl,
  ThreadMetadataRefinementCoordinator,
} from "@artisan/backend";
import {
  ClaudeEngine,
  CodexEngine,
  CodexProcessFactoryLive,
  CursorEngine,
  EngineProcessError,
  EngineToolchain,
  GrokEngine,
  HermesEngine,
  OpenCode2Engine,
  make_claude_engine_layer,
  make_codex_engine_layer,
  make_cursor_engine_layer,
  make_engine_toolchain_layer,
  make_grok_engine_layer,
  make_hermes_engine_layer,
  make_opencode2_engine_layer,
  NodeToolchainReleaseHttpLive,
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

export const ForgeWebSocketBinding: ForgeTransportBindingService = {
  Bind: BindForgeWebSocket,
};

/** Leaves room for the native CLI's 15-second lifecycle deadline to observe exit. */
const forge_shutdown_drain_timeout = "12 seconds";

export interface ForgeShutdownResources {
  readonly close_scope: Effect.Effect<void>;
  readonly drains: ReadonlyArray<Effect.Effect<void>>;
  readonly release_lease: Effect.Effect<void, unknown>;
  readonly timeout?: Duration.Input;
}

/** Drains all runtime-owned work before releasing the hosting scope exactly once. */
export const MakeForgeClose = ({
  close_scope,
  drains,
  release_lease,
  timeout = forge_shutdown_drain_timeout,
}: ForgeShutdownResources) =>
  Effect.cached(
    Effect.all(drains, { concurrency: "unbounded", discard: true }).pipe(
      Effect.timeout(timeout),
      Effect.ignore,
      Effect.ensuring(release_lease.pipe(Effect.ignore)),
      Effect.ensuring(close_scope),
    ),
  );

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
    const orchestrator = yield* AgentOrchestrator;
    const graph_orchestrator = yield* AgentGraphOrchestrator;
    const metadata_refinement = yield* ThreadMetadataRefinementCoordinator;
    /** Resolve the scoped coordinator so its durable watcher stays owned by this host. */
    void metadata_refinement;
    const authority = yield* ForgeControlAuthority;
    const http = yield* Effect.acquireRelease(
      start_forge_http(config, authority, {
        ActiveWorkCount: Effect.gen(function* () {
          const [native_runs, graph_runs] = yield* Effect.all([
            orchestrator.ActiveRunCount,
            graph_orchestrator.ActiveRunCount,
          ]);

          return native_runs + graph_runs;
        }),
      }),
      (server) => server.Close.pipe(Effect.ignore),
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
            Effect.logWarning(`Forge instance card was not announced: ${String(failure.cause)}`),
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
      DrainForShutdown: Effect.all(
        [orchestrator.DrainForShutdown, graph_orchestrator.DrainForShutdown],
        { concurrency: "unbounded", discard: true },
      ),
      endpoint: http.endpoint,
      ReleaseLease: lease.Release,
      RequestShutdown: authority.RequestShutdown,
      ShutdownRequested: authority.ShutdownRequested,
    };
  }).pipe(Effect.onError(() => Effect.logError("Artisan Forge host acquisition failed")));

export interface ForgeRuntimeTelemetryOptions {
  readonly product_telemetry?: Layer.Layer<ProductTelemetry>;
  readonly telemetry_preferences?: Layer.Layer<TelemetryPreferencesControl>;
}

const MakeForgeRuntime = (config: ForgeConfig, telemetry: ForgeRuntimeTelemetryOptions) => {
  const toolchain_root = join(dirname(config.database_path), "toolchain");
  const codex_home = join(toolchain_root, "codex", "home");
  const claude_home = join(toolchain_root, "claude", "home");
  /**
   * One toolchain instance serves both the engine spawn overrides and the
   * protocol handlers; layer memoization by reference keeps the install
   * activity every consumer observes in a single place.
   */
  const toolchain_layer = make_engine_toolchain_layer({
    root: toolchain_root,
  }).pipe(
    Layer.provide(NodeToolchainReleaseHttpLive),
    Layer.provide(CodexProcessFactoryLive),
    Layer.provide(NodeFileSystem.layer),
    Layer.provide(NodePath.layer),
  );
  const engine_layer = Layer.unwrap(
    Effect.gen(function* () {
      const toolchain = yield* EngineToolchain;
      const ManagedSpawn = (engine_id: string) => (profile_id?: string) =>
        toolchain
          .ResolveSpawn(engine_id, profile_id)
          .pipe(Effect.mapError((cause) => new EngineProcessError({ cause, operation: "spawn" })));
      return Layer.mergeAll(
        make_codex_engine_layer({
          ResolveSpawnOverride: ManagedSpawn("codex"),
        }),
        make_claude_engine_layer({
          ResolveSpawnOverride: ManagedSpawn("claude"),
        }),
        make_grok_engine_layer({
          ResolveSpawnOverride: ManagedSpawn("grok"),
        }),
        make_cursor_engine_layer({
          ResolveSpawnOverride: ManagedSpawn("cursor"),
        }),
        make_opencode2_engine_layer({
          ResolveSpawnOverride: ManagedSpawn("opencode2"),
        }),
		make_hermes_engine_layer({
			ResolveSpawnOverride: ManagedSpawn("hermes"),
		}),
      );
    }),
  ).pipe(Layer.provide(toolchain_layer), Layer.provide(CodexProcessFactoryLive));
  const backend_layer = Layer.unwrap(
    Effect.gen(function* () {
      const codex_engine = yield* CodexEngine;
      const claude_engine = yield* ClaudeEngine;
      const grok_engine = yield* GrokEngine;
      const cursor_engine = yield* CursorEngine;
      const opencode2_engine = yield* OpenCode2Engine;
      const hermes_engine = yield* HermesEngine;
      const toolchain = yield* EngineToolchain;
      const codex_executable = make_codex_model_behaviour_executable_layer(
        toolchain.ResolveSpawn("codex").pipe(
          Effect.map((spawn) => spawn.executable),
          Effect.mapError((cause) => new CodexModelBehaviourExecutableUnavailable({ cause })),
        ),
      );
      return make_desktop_backend_layer({
        database_path: config.database_path,
        engine_toolchain: toolchain_layer,
        engines: [
          codex_engine,
          claude_engine,
          opencode2_engine,
          grok_engine,
          cursor_engine,
          hermes_engine,
        ],
        guidance_platform: { codex_home },
        harness_config_platform: { claude_home, codex_home },
        migrations_path: config.migrations_path,
        model_behaviour_platform: { codex_executable, codex_home },
        ...(telemetry.product_telemetry === undefined
          ? {}
          : { product_telemetry: telemetry.product_telemetry }),
        ...(telemetry.telemetry_preferences === undefined
          ? {}
          : { telemetry_preferences: telemetry.telemetry_preferences }),
      });
    }),
  ).pipe(Layer.provide(engine_layer), Layer.provide(toolchain_layer));
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
  telemetry: ForgeRuntimeTelemetryOptions = {},
) =>
  Effect.gen(function* () {
    const file_system = yield* FileSystem.FileSystem;
    const path = yield* Path.Path;
    yield* file_system.makeDirectory(path.dirname(config.database_path), { recursive: true });
    const host_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
      Scope.close(scope, Exit.void),
    );
    const runtime = yield* Layer.build(MakeForgeRuntime(config, telemetry)).pipe(
      Effect.provideService(Scope.Scope, host_scope),
    );
    const host = yield* MakeForgeHost(config, transport_binding).pipe(
      Effect.provide(runtime),
      Effect.provideService(Scope.Scope, host_scope),
    );
    const Close = yield* MakeForgeClose({
      close_scope: Scope.close(host_scope, Exit.void),
      drains: [host.DrainForShutdown],
      release_lease: host.ReleaseLease,
    });
    return {
      endpoint: host.endpoint,
      RequestShutdown: host.RequestShutdown,
      ShutdownRequested: host.ShutdownRequested,
      Close,
    } satisfies ForgeHandle;
  }).pipe(Effect.provide(NodeFileSystem.layer), Effect.provide(NodePath.layer));
