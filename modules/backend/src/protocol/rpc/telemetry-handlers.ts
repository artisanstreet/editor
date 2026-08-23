import { Effect, Option } from "effect";

import type {
  TelemetryIntentCaptureEnvelope,
  TelemetryPreferencesQueryEnvelope,
  TelemetryPreferencesUpdateEnvelope,
} from "@artisan/protocol";

import { RuntimeMetadata } from "../../runtime/metadata";
import { ProductTelemetry } from "../../telemetry/product-telemetry";
import { TelemetryPreferencesControl } from "../../telemetry/preferences-control";
import { ReadyConnectionRuntime } from "./ready-mutations";
import type { ReadyState } from "../connection-state";

export const MakeTelemetryControlHandlers = Effect.gen(function* () {
  const metadata = yield* RuntimeMetadata;
  const product_telemetry = Option.getOrElse(yield* Effect.serviceOption(ProductTelemetry), () => ({
    Capture: () => Effect.void,
  }));
  const preferences = Option.getOrElse(
    yield* Effect.serviceOption(TelemetryPreferencesControl),
    () => ({
      Read: Effect.succeed({
        crash_reports: "unset" as const,
        usage_analytics: "unset" as const,
        version: 1 as const,
      }),
      Update: (patch: {
        crash_reports?: "disabled" | "enabled" | "unset";
        usage_analytics?: "disabled" | "enabled" | "unset";
      }) =>
        Effect.succeed({
          crash_reports: patch.crash_reports ?? "unset",
          usage_analytics: patch.usage_analytics ?? "unset",
          version: 1 as const,
        }),
    }),
  );
  const { Enqueue, EnqueueError } = yield* ReadyConnectionRuntime;

  const Result = <Kind extends string, Payload>(
    query: { readonly message_id: string },
    kind: Kind,
    payload: Payload,
  ) =>
    Effect.gen(function* () {
      const message_id = yield* metadata.MakeId("message");
      const sent_at = yield* metadata.Now;
      return {
        correlation_id: query.message_id,
        kind,
        message_id,
        origin: "backend" as const,
        payload,
        protocol_version: 1 as const,
        schema_version: 1 as const,
        sent_at,
      };
    });

  const HandleFailure = (current: ReadyState, correlation_id: string) =>
    EnqueueError(
      current,
      "telemetry.preferences.unavailable",
      "Artisan could not read or update the telemetry preferences file.",
      true,
      correlation_id,
    );

  return {
    HandleCapture: (envelope: TelemetryIntentCaptureEnvelope, _current: ReadyState) =>
      Effect.gen(function* () {
        const event =
          envelope.payload.event === "feature_used"
            ? {
                event: "feature_used" as const,
                properties: { feature: envelope.payload.feature },
              }
            : envelope.payload.event === "editor_session_started"
              ? {
                  event: "editor_session_started" as const,
                  properties: {
                    forge_connection: envelope.payload.forge_connection,
                    surface: envelope.payload.surface,
                    time_to_ready_ms: envelope.payload.time_to_ready_ms,
                  },
                }
              : {
                  event: "forge_connection_finished" as const,
                  properties: {
                    attempt: envelope.payload.attempt,
                    duration_ms: envelope.payload.duration_ms,
                    ...(envelope.payload.outcome === "failed"
                      ? {
                          failure_code:
                            envelope.payload.failure_code === "authentication_failed"
                              ? ("authentication" as const)
                              : envelope.payload.failure_code === "protocol_mismatch"
                                ? ("protocol" as const)
                                : envelope.payload.failure_code === "unavailable"
                                  ? ("engine_unavailable" as const)
                                  : envelope.payload.failure_code === "transport_error"
                                    ? ("network" as const)
                                    : envelope.payload.failure_code,
                        }
                      : {}),
                    outcome: envelope.payload.outcome,
                  },
                };
        yield* product_telemetry.Capture(event, `renderer_intent:${envelope.message_id}`);
        const result = yield* Result(envelope, "telemetry.intent.capture.result" as const, {
          accepted: true as const,
        });
        yield* Enqueue(result);
      }),
    HandleQuery: (envelope: TelemetryPreferencesQueryEnvelope, current: ReadyState) =>
      Effect.gen(function* () {
        const payload = yield* preferences.Read;
        const result = yield* Result(
          envelope,
          "telemetry.preferences.query.result" as const,
          payload,
        );
        yield* Enqueue(result);
      }).pipe(Effect.catch(() => HandleFailure(current, envelope.message_id))),
    HandleUpdate: (envelope: TelemetryPreferencesUpdateEnvelope, current: ReadyState) =>
      Effect.gen(function* () {
        const payload = yield* preferences.Update({
          ...(envelope.payload.crash_reports === undefined
            ? {}
            : { crash_reports: envelope.payload.crash_reports }),
          ...(envelope.payload.usage_analytics === undefined
            ? {}
            : { usage_analytics: envelope.payload.usage_analytics }),
        });
        const result = yield* Result(
          envelope,
          "telemetry.preferences.update.result" as const,
          payload,
        );
        yield* Enqueue(result);
      }).pipe(Effect.catch(() => HandleFailure(current, envelope.message_id))),
  } as const;
});
