import { createHash } from "node:crypto";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { MakeTelemetryPreferencesStore } from "../../modules/forge/src/telemetry/preferences";
import { MakePostHogTelemetry } from "../../modules/forge/src/telemetry/posthog";

const preferences = (choice: "unset" | "enabled" | "disabled") => {
  const path = join(mkdtempSync(join(tmpdir(), "artisan-posthog-")), "telemetry.json");
  writeFileSync(
    path,
    JSON.stringify({
      crash_reports: "disabled",
      installation_id: "123e4567-e89b-42d3-a456-426614174000",
      updated_at: "2026-08-22T12:00:00.000Z",
      usage_analytics: choice,
      version: 1,
    }),
  );
  return MakeTelemetryPreferencesStore(path);
};

const safe_event = {
  event: "run_finished",
  properties: { duration_ms: 1200, outcome: "completed" },
} as const;

const decode = (input: unknown) => (input === safe_event ? safe_event : undefined);
const metadata = {
  app_version: "1.2.3",
  arch: "x64" as const,
  environment: "production" as const,
  forge_mode: "local" as const,
  is_packaged: true,
  platform: "windows" as const,
  release: "artisan-forge@1.2.3+abc123",
  release_channel: "stable" as const,
  surface: "forge" as const,
};

describe("Forge PostHog telemetry", () => {
  it("does not construct a client or send for unset, disabled, or development", async () => {
    for (const [choice, environment] of [
      ["unset", "production"],
      ["disabled", "production"],
      ["enabled", "development"],
    ] as const) {
      const factory = vi.fn();
      const telemetry = MakePostHogTelemetry({
        client_factory: factory,
        decode_event: decode,
        metadata: { ...metadata, environment },
        preferences: preferences(choice),
        project_key: "public-project-key",
      });

      await expect(telemetry.capture(safe_event, "run_1:finished")).resolves.toBeUndefined();
      expect(factory).not.toHaveBeenCalled();
    }
  });

  it("sends one allowlisted event with safe common properties and deterministic dedupe", async () => {
    const capture = vi.fn();
    const factory = vi.fn(() => ({ capture, shutdown: vi.fn(async () => undefined) }));
    const telemetry = MakePostHogTelemetry({
      client_factory: factory,
      decode_event: decode,
      metadata,
      preferences: preferences("enabled"),
      project_key: "public-project-key",
    });

    await telemetry.capture(safe_event, "run_1:finished");

    const expected_insert_id = createHash("sha256")
      .update("123e4567-e89b-42d3-a456-426614174000\0run_1:finished")
      .digest("hex");
    expect(capture).toHaveBeenCalledOnce();
    expect(capture).toHaveBeenCalledWith({
      distinctId: "install_123e4567-e89b-42d3-a456-426614174000",
      event: "run_finished",
      properties: {
        $geoip_disable: true,
        $insert_id: expected_insert_id,
        app_version: "1.2.3",
        arch: "x64",
        duration_ms: 1200,
        environment: "production",
        event_schema_version: 1,
        forge_mode: "local",
        is_packaged: true,
        outcome: "completed",
        platform: "windows",
        release: "artisan-forge@1.2.3+abc123",
        release_channel: "stable",
        surface: "forge",
      },
    });
  });

  it("drops schema failures and isolates client failures from product code", async () => {
    const capture = vi.fn(() => {
      throw new Error("network down");
    });
    const telemetry = MakePostHogTelemetry({
      client_factory: () => ({ capture, shutdown: vi.fn(async () => undefined) }),
      decode_event: decode,
      metadata,
      preferences: preferences("enabled"),
      project_key: "public-project-key",
    });

    await expect(
      telemetry.capture({ event: "prompt_submitted", prompt: "secret" }, "unsafe"),
    ).resolves.toBeUndefined();
    await expect(telemetry.capture(safe_event, "run_2:finished")).resolves.toBeUndefined();
    expect(capture).toHaveBeenCalledOnce();
  });

  it("stops immediately after opt-out and performs a bounded shutdown", async () => {
    const path = join(mkdtempSync(join(tmpdir(), "artisan-posthog-optout-")), "telemetry.json");
    writeFileSync(
      path,
      JSON.stringify({
        crash_reports: "disabled",
        installation_id: "123e4567-e89b-42d3-a456-426614174000",
        updated_at: "2026-08-22T12:00:00.000Z",
        usage_analytics: "enabled",
        version: 1,
      }),
    );
    const store = MakeTelemetryPreferencesStore(path);
    const capture = vi.fn();
    const disable = vi.fn(async () => undefined);
    const shutdown = vi.fn(async () => undefined);
    const telemetry = MakePostHogTelemetry({
      client_factory: () => ({ capture, disable, shutdown }),
      decode_event: decode,
      metadata,
      preferences: store,
      project_key: "public-project-key",
    });

    await telemetry.capture(safe_event, "run_1:finished");
    store.update({ usage_analytics: "disabled" });
    await telemetry.capture(safe_event, "run_2:finished");
    await telemetry.shutdown(100);

    expect(capture).toHaveBeenCalledOnce();
    expect(disable).toHaveBeenCalledOnce();
    expect(shutdown).not.toHaveBeenCalled();
  });
});
