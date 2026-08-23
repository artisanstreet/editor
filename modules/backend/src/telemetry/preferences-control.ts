import { Context, Data, Effect, Layer } from "effect";

import type { TelemetryPreferences, TelemetryPreferencesUpdate } from "@artisan/protocol";

export class TelemetryPreferencesControlError extends Data.TaggedError(
  "TelemetryPreferencesControlError",
)<{
  readonly operation: "read" | "update";
}> {}

export class TelemetryPreferencesControl extends Context.Service<
  TelemetryPreferencesControl,
  {
    readonly Read: Effect.Effect<TelemetryPreferences, TelemetryPreferencesControlError>;
    readonly Update: (
      patch: TelemetryPreferencesUpdate,
    ) => Effect.Effect<TelemetryPreferences, TelemetryPreferencesControlError>;
  }
>()("Artisan/TelemetryPreferencesControl") {}

export const TelemetryPreferencesControlNoop = Layer.succeed(TelemetryPreferencesControl, {
  Read: Effect.succeed({
    crash_reports: "unset" as const,
    usage_analytics: "unset" as const,
    version: 1 as const,
  }),
  Update: (patch) =>
    Effect.succeed({
      crash_reports: patch.crash_reports ?? "unset",
      usage_analytics: patch.usage_analytics ?? "unset",
      version: 1 as const,
    }),
});

export interface TelemetryPreferencesControlPort {
  readonly read: () => Promise<TelemetryPreferences> | TelemetryPreferences;
  readonly update: (
    patch: TelemetryPreferencesUpdate,
  ) => Promise<TelemetryPreferences> | TelemetryPreferences;
}

export const make_telemetry_preferences_control_layer = (port: TelemetryPreferencesControlPort) =>
  Layer.succeed(TelemetryPreferencesControl, {
    Read: Effect.tryPromise({
      try: () => Promise.resolve(port.read()),
      catch: () => new TelemetryPreferencesControlError({ operation: "read" }),
    }),
    Update: (patch) =>
      Effect.tryPromise({
        try: () => Promise.resolve(port.update(patch)),
        catch: () => new TelemetryPreferencesControlError({ operation: "update" }),
      }),
  });
