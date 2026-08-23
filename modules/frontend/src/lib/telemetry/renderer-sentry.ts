import { Cause, Context, Duration, Effect, Layer, Option, Scope, Stream } from "effect";

import { SanitizeSentryEvent } from "@artisan/observability";
import { TelemetryController } from "../settings/telemetry-controller";

declare const __ARTISAN_RELEASE_COMMIT__: string;
declare const __ARTISAN_RELEASE_VERSION__: string;
declare const __ARTISAN_SENTRY_EDITOR_DSN__: string;
declare const __ARTISAN_TELEMETRY_ENVIRONMENT__: string;

const BuildValue = (value: string | undefined) =>
  value === undefined || value.trim() === "" ? undefined : value.trim();

const dsn = BuildValue(
  typeof __ARTISAN_SENTRY_EDITOR_DSN__ === "undefined" ? undefined : __ARTISAN_SENTRY_EDITOR_DSN__,
);
const environment = BuildValue(
  typeof __ARTISAN_TELEMETRY_ENVIRONMENT__ === "undefined"
    ? undefined
    : __ARTISAN_TELEMETRY_ENVIRONMENT__,
);
const release_version =
  BuildValue(
    typeof __ARTISAN_RELEASE_VERSION__ === "undefined" ? undefined : __ARTISAN_RELEASE_VERSION__,
  ) ?? "development";
const release_commit =
  BuildValue(
    typeof __ARTISAN_RELEASE_COMMIT__ === "undefined" ? undefined : __ARTISAN_RELEASE_COMMIT__,
  ) ?? "development";

export class RendererErrorMonitoring extends Context.Service<
  RendererErrorMonitoring,
  { readonly initialized: boolean }
>()("Artisan/RendererErrorMonitoring") {}

export const RendererErrorMonitoringLive = Layer.effect(
  RendererErrorMonitoring,
  Effect.gen(function* () {
    const telemetry = yield* TelemetryController;
    const layer_scope = yield* Effect.scope;
    const StartMonitoring = Effect.gen(function* () {
      let preferences = yield* telemetry.Current;
      yield* telemetry.Changes.pipe(
        Stream.runForEach((next) =>
          Effect.sync(() => {
            preferences = next;
          }),
        ),
        Effect.forkIn(layer_scope),
      );
      preferences = yield* telemetry.Refresh.pipe(Effect.catchCause(() => telemetry.Current));
      if (dsn === undefined || (environment !== "production" && environment !== "staging")) {
        return;
      }
      const Initialize = Effect.gen(function* () {
        if (preferences.crash_reports !== "enabled") {
          const enabled = yield* telemetry.Changes.pipe(
            Stream.filter((next) => next.crash_reports === "enabled"),
            Stream.runHead,
          );
          if (Option.isNone(enabled)) return yield* Effect.interrupt;
          preferences = enabled.value;
        }

        const sentry = yield* Effect.tryPromise(() => import("@sentry/browser"));
        if (preferences.crash_reports !== "enabled") {
          return yield* Effect.fail("crash-reporting-disabled");
        }
        sentry.init({
          attachStacktrace: true,
          beforeBreadcrumb: () => null,
          beforeSend: (event) =>
            preferences.crash_reports === "enabled"
              ? (SanitizeSentryEvent(event) as typeof event | null)
              : null,
          defaultIntegrations: false,
          dsn,
          environment,
          maxBreadcrumbs: 0,
          release: `artisan-editor@${release_version}+${release_commit}`,
          sendClientReports: false,
          sendDefaultPii: false,
          tracesSampleRate: 0,
        });

        const CaptureError = (event: ErrorEvent) => {
          if (preferences.crash_reports === "enabled") {
            sentry.captureException(event.error ?? new Error("Renderer error"));
          }
        };
        const CaptureRejection = (event: PromiseRejectionEvent) => {
          if (preferences.crash_reports === "enabled") {
            sentry.captureException(
              event.reason instanceof Error ? event.reason : new Error("Renderer rejection"),
            );
          }
        };
        globalThis.addEventListener("error", CaptureError);
        globalThis.addEventListener("unhandledrejection", CaptureRejection);
        yield* Scope.addFinalizer(
          layer_scope,
          Effect.promise(async () => {
            globalThis.removeEventListener("error", CaptureError);
            globalThis.removeEventListener("unhandledrejection", CaptureRejection);
            await sentry.close(500);
          }),
        );
      });
      const InitializeWithRetry: Effect.Effect<void, unknown> = Effect.suspend(() =>
        Initialize.pipe(
          Effect.catchCause((cause) =>
            Cause.hasInterrupts(cause)
              ? Effect.failCause(cause)
              : Effect.sleep(Duration.seconds(2)).pipe(Effect.andThen(InitializeWithRetry)),
          ),
        ),
      );
      yield* InitializeWithRetry;
    });
    yield* Effect.forkIn(StartMonitoring, layer_scope);
    return RendererErrorMonitoring.of({ initialized: false });
  }),
);
