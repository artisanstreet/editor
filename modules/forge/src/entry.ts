import { SnowflakeId } from "@artisan/protocol";
import { dirname, resolve } from "node:path";
import { Clock, Console, Duration, Effect, Schema } from "effect";

import { decode_forge_config } from "./config";
import { ForgeWebSocketBinding, StartForge } from "./forge-host";
import { ResolveInstanceRegistryRoot } from "./instance-registry";
import { MaintainBoundedForgeLog } from "./log-retention";
import { WatchForgeMemory } from "./memory-telemetry";
import { RemoveForgeState, WriteForgeState } from "./state";
import { EvaluateArtisanBroker } from "./broker";
import { StartForgeCrashMarker } from "./telemetry/crash-marker";
import { ForgeBuildTelemetryConfig, MakeForgeTelemetryRuntime } from "./telemetry/runtime";

const ForgeParentMessage = Schema.Struct({
  kind: Schema.Literal("artisan:forge-shutdown"),
});

const RequiredEnvironment = (name: string) =>
  Effect.gen(function* () {
    const value = process.env[name];
    if (value === undefined || value.length === 0) {
      return yield* Effect.fail(new Error(`Artisan Forge requires ${name}`));
    }
    return value;
  });

const AwaitProcessShutdown = Effect.callback<"parent_disconnect" | "signal">((resume) => {
  const signal = () => resume(Effect.succeed("signal" as const));
  const parent_disconnect = () => resume(Effect.succeed("parent_disconnect" as const));
  const message = (value: unknown) => {
    if (Schema.is(ForgeParentMessage)(value)) parent_disconnect();
  };

  process.once("SIGINT", signal);
  process.once("SIGTERM", signal);
  if (process.connected) process.once("disconnect", parent_disconnect);
  process.on("message", message);

  return Effect.sync(() => {
    process.removeListener("SIGINT", signal);
    process.removeListener("SIGTERM", signal);
    process.removeListener("disconnect", parent_disconnect);
    process.removeListener("message", message);
  });
});

/** Runs the headless Forge lifecycle inside the executable's single Effect runtime. */
export const StartForgeFromEnvironment = Effect.gen(function* () {
  yield* EvaluateArtisanBroker();
  const snowflake_id = yield* SnowflakeId;
  const instance_id = process.env.ARTISAN_FORGE_INSTANCE_ID ?? (yield* snowflake_id.Make("forge"));
  const database_path = yield* RequiredEnvironment("ARTISAN_DATABASE_PATH");
  const migrations_path = yield* RequiredEnvironment("ARTISAN_MIGRATIONS_PATH");
  const telemetry_config_path = process.env.ARTISAN_TELEMETRY_CONFIG_PATH;
  const started_at_ms = yield* Clock.currentTimeMillis;
  const started_at = new Date(started_at_ms).toISOString();
  const crash_marker =
    telemetry_config_path === undefined
      ? undefined
      : yield* Effect.sync(() =>
          StartForgeCrashMarker({
            commit: process.env.ARTISAN_RELEASE_COMMIT ?? "development",
            marker_path: resolve(dirname(telemetry_config_path), "forge-crash.json"),
            release: `artisan-forge@${process.env.ARTISAN_RELEASE_VERSION ?? "development"}+${process.env.ARTISAN_RELEASE_COMMIT ?? "development"}`,
            started_at,
          }),
        ).pipe(Effect.catchCause(() => Effect.succeed(undefined)));
  const release_version = process.env.ARTISAN_RELEASE_VERSION ?? "development";
  const release_commit = process.env.ARTISAN_RELEASE_COMMIT ?? "development";
  const telemetry = MakeForgeTelemetryRuntime({
    app_version: release_version,
    crash_reports_dsn: process.env.ARTISAN_SENTRY_FORGE_DSN ?? ForgeBuildTelemetryConfig.sentry_dsn,
    environment: process.env.ARTISAN_TELEMETRY_ENVIRONMENT ?? ForgeBuildTelemetryConfig.environment,
    forge_mode: process.env.ARTISAN_FORGE_MODE === "headless" ? "headless" : "local",
    posthog_host: process.env.ARTISAN_POSTHOG_HOST ?? ForgeBuildTelemetryConfig.posthog_host,
    posthog_project_key:
      process.env.ARTISAN_POSTHOG_PROJECT_KEY ?? ForgeBuildTelemetryConfig.posthog_project_key,
    previous_exit: crash_marker?.previous_exit ?? "unknown",
    release: `artisan-forge@${release_version}+${release_commit}`,
    release_channel:
      process.env.ARTISAN_RELEASE_CHANNEL === "stable"
        ? "stable"
        : process.env.ARTISAN_RELEASE_CHANNEL === "beta"
          ? "beta"
          : "development",
    telemetry_config_path,
  });
  const instance_registry_root =
    process.env.ARTISAN_INSTANCE_REGISTRY_ROOT ?? ResolveInstanceRegistryRoot(process.env);
  const host = yield* StartForge(
    decode_forge_config({
      database_path,
      instance_id,
      ...(instance_registry_root === undefined ? {} : { instance_registry_root }),
      listen_host: process.env.ARTISAN_LISTEN_HOST === "::1" ? "::1" : "127.0.0.1",
      listen_port: process.env.ARTISAN_LISTEN_PORT ? Number(process.env.ARTISAN_LISTEN_PORT) : 0,
      migrations_path,
      ...(process.env.ARTISAN_ALLOWED_HOSTNAMES === undefined
        ? {}
        : {
            allowed_hostnames: process.env.ARTISAN_ALLOWED_HOSTNAMES.split(",")
              .map((hostname) => hostname.trim().toLowerCase())
              .filter(Boolean),
          }),
      ...(process.env.ARTISAN_ALLOWED_ORIGINS === undefined
        ? {}
        : {
            allowed_origins: process.env.ARTISAN_ALLOWED_ORIGINS.split(",")
              .map((origin) => origin.trim())
              .filter(Boolean),
          }),
      ...(process.env.ARTISAN_AUTH_TOKEN === undefined
        ? {}
        : { auth_token: process.env.ARTISAN_AUTH_TOKEN }),
      ...(process.env.ARTISAN_FORGE_DEVELOPMENT === "1" ? { development: true } : {}),
      ...(process.env.ARTISAN_STATIC_FRONTEND_ROOT === undefined
        ? {}
        : {
            static_frontend_root: process.env.ARTISAN_STATIC_FRONTEND_ROOT,
          }),
    }),
    ForgeWebSocketBinding,
    {
      product_telemetry: telemetry.product_telemetry,
      telemetry_preferences: telemetry.telemetry_preferences,
    },
  );
  yield* Effect.tryPromise(() =>
    telemetry.capture(
      {
        event: "forge_started",
        properties: {
          cold_start_duration_ms: Math.min(604_800_000, Math.max(0, Date.now() - started_at_ms)),
          forge_mode: process.env.ARTISAN_FORGE_MODE === "headless" ? "headless" : "local",
          previous_exit: crash_marker?.previous_exit ?? "unknown",
        },
      },
      `forge_started:${process.pid}:${started_at_ms}`,
    ),
  ).pipe(Effect.catchCause(() => Effect.void));
  const state_path = process.env.ARTISAN_FORGE_STATE_PATH;
  const log_path = process.env.ARTISAN_FORGE_LOG_PATH;
  if (log_path !== undefined) {
    yield* Effect.forkScoped(MaintainBoundedForgeLog(log_path));
  }

  let shutdown_reason: "parent_disconnect" | "requested" | "signal" = "requested";
  yield* Effect.acquireUseRelease(
    Effect.gen(function* () {
      if (state_path !== undefined) {
        const now = yield* Clock.currentTimeMillis;
        yield* WriteForgeState(state_path, {
          endpoint: host.endpoint.toString(),
          instance_id,
          pid: process.pid,
          started_at: new Date(now).toISOString(),
          version: 1,
        });
      }

      yield* Console.log(
        JSON.stringify({
          endpoint: host.endpoint.toString(),
          kind: "artisan:forge-ready",
          pid: process.pid,
        }),
      );

      /**
       * Left on permanently. Forge has died of memory exhaustion repeatedly
       * with nothing in the log to say what it was holding, and every
       * diagnosis so far has been inference from the outside.
       */
      yield* Effect.forkScoped(WatchForgeMemory);
      if (crash_marker !== undefined) {
        yield* Effect.forkScoped(
          Effect.sleep(Duration.seconds(30)).pipe(
            Effect.andThen(
              Effect.sync(() =>
                crash_marker.heartbeat({
                  at: new Date().toISOString(),
                  memory_bytes: process.memoryUsage().rss,
                }),
              ),
            ),
            Effect.forever,
          ),
        );
      }

      return host;
    }),
    (current_host) =>
      Effect.raceFirst(
        current_host.ShutdownRequested.pipe(Effect.as("requested" as const)),
        AwaitProcessShutdown,
      ).pipe(
        Effect.tap((reason) =>
          Effect.sync(() => {
            shutdown_reason = reason;
          }),
        ),
      ),
    (current_host) =>
      Effect.gen(function* () {
        yield* current_host.Close;
        yield* Effect.tryPromise(() =>
          telemetry.capture(
            {
              event: "forge_stopped",
              properties: {
                shutdown_reason,
                uptime_ms: Math.min(604_800_000, Math.max(0, Date.now() - started_at_ms)),
              },
            },
            `forge_stopped:${process.pid}:${started_at_ms}`,
          ),
        ).pipe(Effect.catchCause(() => Effect.void));
        yield* Effect.tryPromise(() => telemetry.shutdown()).pipe(
          Effect.catchCause(() => Effect.void),
        );
        if (state_path !== undefined) {
          yield* RemoveForgeState(state_path, instance_id);
        }
        crash_marker?.mark_clean(new Date().toISOString());
        process.exitCode = 0;
      }),
  );
}).pipe(
  Effect.scoped,
  Effect.ensuring(
    Effect.sync(() => {
      if (process.connected) process.disconnect?.();
    }),
  ),
);
