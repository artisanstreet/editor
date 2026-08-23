import { Context, Effect, Layer, Semaphore, Stream, SubscriptionRef } from "effect";

import type {
  TelemetryIntent,
  TelemetryPreference,
  TelemetryPreferences,
  TelemetryPreferencesUpdate,
} from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const InitialTelemetryPreferences: TelemetryPreferences = {
  crash_reports: "unset",
  usage_analytics: "unset",
  version: 1,
};

export class TelemetryController extends Context.Service<
  TelemetryController,
  {
    readonly Capture: (intent: TelemetryIntent) => Effect.Effect<void, ArtisanClientError>;
    readonly Changes: Stream.Stream<TelemetryPreferences>;
    readonly Current: Effect.Effect<TelemetryPreferences>;
    readonly Refresh: Effect.Effect<TelemetryPreferences, ArtisanClientError>;
    readonly SetCrashReports: (
      choice: TelemetryPreference,
    ) => Effect.Effect<TelemetryPreferences, ArtisanClientError>;
    readonly SetUsageAnalytics: (
      choice: TelemetryPreference,
    ) => Effect.Effect<TelemetryPreferences, ArtisanClientError>;
  }
>()("Artisan/TelemetryController") {}

export const TelemetryControllerLive = Layer.effect(
  TelemetryController,
  Effect.gen(function* () {
    const client = yield* ArtisanClient;
    const state = yield* SubscriptionRef.make(InitialTelemetryPreferences);
    const mutation_lock = yield* Semaphore.make(1);
    const Current = SubscriptionRef.get(state);
    const GetPreferences =
      client.GetTelemetryPreferences ?? Effect.succeed(InitialTelemetryPreferences);
    const Refresh = mutation_lock.withPermit(
      GetPreferences.pipe(Effect.tap((preferences) => SubscriptionRef.set(state, preferences))),
    );
    const Update = (update: TelemetryPreferencesUpdate) =>
      mutation_lock.withPermit(
        (
          client.UpdateTelemetryPreferences?.(update) ??
          SubscriptionRef.get(state).pipe(
            Effect.map((current) => ({
              ...current,
              ...(update.crash_reports === undefined
                ? {}
                : { crash_reports: update.crash_reports }),
              ...(update.usage_analytics === undefined
                ? {}
                : { usage_analytics: update.usage_analytics }),
            })),
          )
        ).pipe(Effect.tap((preferences) => SubscriptionRef.set(state, preferences))),
      );
    return TelemetryController.of({
      Capture: client.CaptureTelemetryIntent ?? (() => Effect.void),
      Changes: SubscriptionRef.changes(state),
      Current,
      Refresh,
      SetCrashReports: (crash_reports) => Update({ crash_reports }),
      SetUsageAnalytics: (usage_analytics) => Update({ usage_analytics }),
    });
  }),
);
