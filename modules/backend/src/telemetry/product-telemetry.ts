import { Context, Effect, Layer } from "effect";

import { DecodeProductTelemetryEvent, type ProductTelemetryEvent } from "@artisan/observability";

export interface ProductTelemetryAdapter {
  readonly capture: (event: ProductTelemetryEvent, canonical_event_id: string) => Promise<unknown>;
}

export class ProductTelemetry extends Context.Service<
  ProductTelemetry,
  {
    readonly Capture: (
      event: ProductTelemetryEvent,
      canonical_event_id: string,
    ) => Effect.Effect<void>;
  }
>()("Artisan/ProductTelemetry") {}

export const ProductTelemetryNoop = Layer.succeed(ProductTelemetry, {
  Capture: () => Effect.void,
});

/** Vendor-neutral adapter boundary; validation and all failures are contained here. */
export const make_product_telemetry_layer = (adapter: ProductTelemetryAdapter) =>
  Layer.succeed(ProductTelemetry, {
    Capture: (input, canonical_event_id) =>
      Effect.gen(function* () {
        const event = yield* Effect.try({
          try: () => DecodeProductTelemetryEvent(input),
          catch: () => undefined,
        });
        if (event === undefined) return;
        yield* Effect.tryPromise({
          try: () => adapter.capture(event, canonical_event_id),
          catch: () => undefined,
        });
      }).pipe(Effect.catchCause(() => Effect.void)),
  });
