import { Effect } from "effect";
import { describe, expect, it, vi } from "vitest";

import {
  ProductTelemetry,
  ProductTelemetryNoop,
  make_product_telemetry_layer,
} from "@artisan/backend";

describe("ProductTelemetry", () => {
  it("is a total no-op by default", async () => {
    await expect(
      Effect.runPromise(
        Effect.gen(function* () {
          const telemetry = yield* ProductTelemetry;
          yield* telemetry.Capture(
            {
              event: "feature_used",
              properties: { feature: "preview" },
            },
            "feature:preview:1",
          );
        }).pipe(Effect.provide(ProductTelemetryNoop)),
      ),
    ).resolves.toBeUndefined();
  });

  it("validates before dispatch and isolates adapter failures", async () => {
    const capture = vi
      .fn()
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("network down"));
    const layer = make_product_telemetry_layer({ capture });
    const program = Effect.gen(function* () {
      const telemetry = yield* ProductTelemetry;
      yield* telemetry.Capture(
        { event: "feature_used", properties: { feature: "preview" } },
        "feature:preview:1",
      );
      yield* telemetry.Capture(
        { event: "feature_used", properties: { feature: "thread_search" } },
        "feature:search:1",
      );
      yield* telemetry.Capture(
        { event: "prompt_submitted", properties: { prompt: "private" } } as never,
        "unsafe",
      );
    }).pipe(Effect.provide(layer));

    await expect(Effect.runPromise(program)).resolves.toBeUndefined();
    expect(capture).toHaveBeenCalledTimes(2);
    expect(capture).toHaveBeenNthCalledWith(
      1,
      { event: "feature_used", properties: { feature: "preview" } },
      "feature:preview:1",
    );
  });
});
