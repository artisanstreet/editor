import { Effect, Layer } from "effect";

import { RuntimeSurfaceFor } from "../browser/runtime-surface";
import { TelemetryController } from "../settings/telemetry-controller";

const renderer_started_at = Date.now();

/** One canonical renderer lifecycle event; transport retries cannot duplicate it. */
export const ProductTelemetryBootstrapLive = Layer.effectDiscard(
  Effect.gen(function* () {
    const telemetry = yield* TelemetryController;
    const surface = RuntimeSurfaceFor(globalThis.navigator?.userAgent ?? "");
    yield* telemetry
      .Capture({
        event: "editor_session_started",
        forge_connection: surface === "desktop" ? "local" : "remote",
        surface: surface === "desktop" ? "desktop_renderer" : "browser_renderer",
        time_to_ready_ms: Math.min(600_000, Math.max(0, Date.now() - renderer_started_at)),
      })
      .pipe(
        Effect.catchCause(() => Effect.void),
        Effect.forkScoped,
      );
  }),
);
