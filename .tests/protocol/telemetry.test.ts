import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
  ControlRpcGroup,
  DecodeInboundControlEnvelope,
  DecodeOutboundControlEnvelope,
  DecodeTelemetryIntent,
  DecodeTelemetryPreferencesUpdate,
} from "@artisan/protocol";

describe("telemetry protocol", () => {
  it("accepts only closed renderer intent names", async () => {
    await expect(
      Effect.runPromise(
        DecodeTelemetryIntent({
          event: "editor_session_started",
          forge_connection: "local",
          surface: "desktop_renderer",
          time_to_ready_ms: 850,
        }),
      ),
    ).resolves.toMatchObject({ event: "editor_session_started" });

    await expect(
      Effect.runPromise(
        DecodeTelemetryIntent({
          event: "prompt_submitted",
          prompt: "private prompt",
        }),
      ),
    ).rejects.toBeDefined();
  });

  it("bounds connection intents and never admits content-bearing fields", async () => {
    const valid = {
      attempt: "reconnect",
      duration_ms: 1_250,
      event: "forge_connection_finished",
      failure_code: "timeout",
      outcome: "failed",
    } as const;
    await expect(Effect.runPromise(DecodeTelemetryIntent(valid))).resolves.toEqual(valid);

    for (const invalid of [
      { ...valid, duration_ms: 600_001 },
      { ...valid, failure_code: "C:/Users/sander/private.txt" },
      { ...valid, prompt: "ARTISAN_TELEMETRY_FORBIDDEN_CANARY" },
      { ...valid, token: "Bearer secret" },
      { ...valid, url: "https://example.test/private?token=secret" },
    ]) {
      await expect(Effect.runPromise(DecodeTelemetryIntent(invalid))).rejects.toBeDefined();
    }
  });

  it("allows only fixed renderer feature names", async () => {
    await expect(
      Effect.runPromise(
        DecodeTelemetryIntent({ event: "feature_used", feature: "workspace_review" }),
      ),
    ).resolves.toEqual({ event: "feature_used", feature: "workspace_review" });
    await expect(
      Effect.runPromise(
        DecodeTelemetryIntent({ event: "feature_used", feature: "my_private_plugin" }),
      ),
    ).rejects.toBeDefined();
  });

  it("accepts only non-empty tri-state preference updates", async () => {
    await expect(
      Effect.runPromise(
        DecodeTelemetryPreferencesUpdate({
          crash_reports: "disabled",
          usage_analytics: "enabled",
        }),
      ),
    ).resolves.toEqual({ crash_reports: "disabled", usage_analytics: "enabled" });

    for (const invalid of [
      {},
      { usage_analytics: "sometimes" },
      { usage_analytics: "enabled", prompt: "private" },
    ]) {
      await expect(
        Effect.runPromise(DecodeTelemetryPreferencesUpdate(invalid)),
      ).rejects.toBeDefined();
    }
  });

  it("wires the preferences query and result as a correlated RPC", async () => {
    const query = {
      kind: "telemetry.preferences.query",
      message_id: "telemetry_query_1",
      origin: "frontend",
      payload: {},
      protocol_version: 1,
      schema_version: 1,
      sent_at: "2026-08-22T12:00:00.000Z",
    } as const;
    const result = {
      correlation_id: query.message_id,
      kind: "telemetry.preferences.query.result",
      message_id: "telemetry_result_1",
      origin: "backend",
      payload: {
        crash_reports: "unset",
        usage_analytics: "disabled",
        version: 1,
      },
      protocol_version: 1,
      schema_version: 1,
      sent_at: "2026-08-22T12:00:00.001Z",
    } as const;

    await expect(Effect.runPromise(DecodeInboundControlEnvelope(query))).resolves.toEqual(query);
    await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).resolves.toEqual(result);
    expect(ControlRpcGroup.requests.has(query.kind)).toBe(true);
  });

  it("wires preference updates without accepting arbitrary settings", async () => {
    const update = {
      kind: "telemetry.preferences.update",
      message_id: "telemetry_update_1",
      origin: "frontend",
      payload: { usage_analytics: "enabled" },
      protocol_version: 1,
      schema_version: 1,
      sent_at: "2026-08-22T12:00:00.000Z",
    } as const;

    await expect(Effect.runPromise(DecodeInboundControlEnvelope(update))).resolves.toEqual(update);
    expect(ControlRpcGroup.requests.has(update.kind)).toBe(true);
    await expect(
      Effect.runPromise(
        DecodeInboundControlEnvelope({
          ...update,
          payload: { usage_analytics: "enabled", repository_path: "C:/private" },
        }),
      ),
    ).rejects.toBeDefined();
  });

  it("wires only typed renderer intents through telemetry.intent.capture", async () => {
    const capture = {
      kind: "telemetry.intent.capture",
      message_id: "telemetry_capture_1",
      origin: "frontend",
      payload: {
        event: "feature_used",
        feature: "preview",
      },
      protocol_version: 1,
      schema_version: 1,
      sent_at: "2026-08-22T12:00:00.000Z",
    } as const;

    await expect(Effect.runPromise(DecodeInboundControlEnvelope(capture))).resolves.toEqual(
      capture,
    );
    expect(ControlRpcGroup.requests.has(capture.kind)).toBe(true);
    await expect(
      Effect.runPromise(
        DecodeInboundControlEnvelope({
          ...capture,
          payload: { event: "custom", properties: { prompt: "private" } },
        }),
      ),
    ).rejects.toBeDefined();
  });
});
