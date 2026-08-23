import { describe, expect, it } from "vitest";

import {
  DecodeProductTelemetryEvent,
  NormalizeCatalogModelId,
  ProductTelemetryEventNames,
} from "@artisan/observability";

describe("product telemetry event catalog", () => {
  it("accepts closed, bounded outcome events", () => {
    expect(
      DecodeProductTelemetryEvent({
        event: "run_finished",
        properties: {
          duration_ms: 1_200,
          engine_id: "codex",
          failure_code: "none",
          model_id: "custom_or_unknown",
          outcome: "completed",
          permission: "standard",
        },
      }),
    ).toEqual({
      event: "run_finished",
      properties: {
        duration_ms: 1_200,
        engine_id: "codex",
        failure_code: "none",
        model_id: "custom_or_unknown",
        outcome: "completed",
        permission: "standard",
      },
    });
    expect(ProductTelemetryEventNames).toContain("run_finished");
  });

  it("rejects arbitrary names, extra properties, content, and unbounded values", () => {
    for (const invalid of [
      { event: "prompt_submitted", properties: { prompt: "private" } },
      {
        event: "run_finished",
        properties: {
          duration_ms: 1,
          engine_id: "codex",
          model_id: "custom_or_unknown",
          outcome: "completed",
          permission: "standard",
          prompt: "private",
        },
      },
      {
        event: "run_finished",
        properties: {
          duration_ms: 10_000_000_000,
          engine_id: "codex",
          model_id: "custom_or_unknown",
          outcome: "completed",
          permission: "standard",
        },
      },
    ]) {
      expect(() => DecodeProductTelemetryEvent(invalid)).toThrow();
    }
  });

  it("normalizes only shipped model ids and never emits custom identifiers", () => {
    expect(NormalizeCatalogModelId("codex-sol")).toBe("codex-sol");
    expect(NormalizeCatalogModelId("my-private-model-at-C:/repo")).toBe("custom_or_unknown");
    expect(NormalizeCatalogModelId(undefined)).toBe("custom_or_unknown");
  });
});
