import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  DecodePostHogHost,
  MakePostHogClientFactory,
} from "../../modules/forge/src/telemetry/posthog-client";
import { MakeForgeTelemetryRuntime } from "../../modules/forge/src/telemetry/runtime";

describe("PostHog client configuration", () => {
  it("admits only the approved HTTPS ingestion hosts", () => {
    expect(DecodePostHogHost(undefined)).toBe("https://eu.i.posthog.com");
    expect(DecodePostHogHost("https://us.i.posthog.com")).toBe("https://us.i.posthog.com");
    expect(() => DecodePostHogHost("http://eu.i.posthog.com")).toThrow();
    expect(() => DecodePostHogHost("https://example.test/private")).toThrow();
  });

  it("constructs a bounded in-memory client without error or AI autocapture", () => {
    const client = MakePostHogClientFactory("https://eu.i.posthog.com")("public-project-key");
    const options = (client as unknown as { readonly options: Record<string, unknown> }).options;

    expect(options).toMatchObject({
      enableExceptionAutocapture: false,
      enableFullAiCapture: false,
      flushAt: 1,
      flushInterval: 0,
      host: "https://eu.i.posthog.com",
      maxQueueSize: 1,
      persistence: "memory",
      privacyMode: true,
    });
  });

  it("disables analytics instead of aborting Forge on an invalid configured host", async () => {
    const telemetry_config_path = join(
      mkdtempSync(join(tmpdir(), "artisan-invalid-posthog-host-")),
      "telemetry.json",
    );
    writeFileSync(
      telemetry_config_path,
      JSON.stringify({
        crash_reports: "disabled",
        installation_id: "0192a5f0-85f8-7abc-9def-0123456789ab",
        updated_at: "2026-08-22T00:00:00.000Z",
        usage_analytics: "enabled",
        version: 1,
      }),
    );
    const runtime = MakeForgeTelemetryRuntime({
      app_version: "1.0.0",
      crash_reports_dsn: undefined,
      environment: "production",
      forge_mode: "local",
      posthog_host: "http://unapproved.invalid",
      posthog_project_key: "public-key",
      previous_exit: "clean",
      release: "artisan-forge@1.0.0+test",
      release_channel: "stable",
      telemetry_config_path,
    });
    await expect(
      runtime.capture({ event: "feature_used", properties: { feature: "preview" } }, "event-1"),
    ).resolves.toBeUndefined();
  });
});
