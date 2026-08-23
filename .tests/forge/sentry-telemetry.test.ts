import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { InitializeForgeSentry } from "../../modules/forge/src/telemetry/sentry";
import { MakeTelemetryPreferencesStore } from "../../modules/forge/src/telemetry/preferences";

const store = (choice: "unset" | "enabled" | "disabled") => {
  const path = join(mkdtempSync(join(tmpdir(), "artisan-sentry-")), "telemetry.json");
  writeFileSync(
    path,
    JSON.stringify({
      crash_reports: choice,
      installation_id: "123e4567-e89b-42d3-a456-426614174000",
      updated_at: "2026-08-22T12:00:00.000Z",
      usage_analytics: "disabled",
      version: 1,
    }),
  );
  return { path, preferences: MakeTelemetryPreferencesStore(path) };
};

const metadata = {
  arch: "x64" as const,
  environment: "production" as const,
  platform: "windows" as const,
  release: "artisan-forge@1.2.3+abc",
  release_channel: "stable" as const,
  runtime: "forge" as const,
};

describe("Forge Sentry", () => {
  it("uses only the minimal node-core integrations in the SEA bundle", () => {
    const source = readFileSync(
      new URL("../../modules/forge/src/telemetry/runtime.ts", import.meta.url),
      "utf8",
    );
    expect(source).toContain("initWithoutDefaultIntegrations");
    expect(source).toContain("onUncaughtExceptionIntegration");
    expect(source).toContain("onUnhandledRejectionIntegration");
    expect(source).not.toContain('from "@sentry/node"');
  });
  it("does not initialize before explicit crash-report consent", () => {
    for (const choice of ["unset", "disabled"] as const) {
      const sdk = { captureMessage: vi.fn(), flush: vi.fn(), init: vi.fn() };
      const runtime = InitializeForgeSentry({
        dsn: "https://public@example.ingest.sentry.io/1",
        metadata,
        preferences: store(choice).preferences,
        sdk,
      });
      expect(runtime.enabled).toBe(false);
      expect(sdk.init).not.toHaveBeenCalled();
    }
  });

  it("initializes with PII, tracing, replay, logs, and unsafe breadcrumbs disabled", () => {
    const sdk = { captureMessage: vi.fn(), flush: vi.fn(), init: vi.fn() };
    const runtime = InitializeForgeSentry({
      dsn: "https://public@example.ingest.sentry.io/1",
      metadata,
      preferences: store("enabled").preferences,
      sdk,
    });

    expect(runtime.enabled).toBe(true);
    expect(sdk.init).toHaveBeenCalledTimes(1);
    const options = sdk.init.mock.calls[0]![0];
    expect(options).toMatchObject({
      attachStacktrace: true,
      dsn: "https://public@example.ingest.sentry.io/1",
      enableLogs: false,
      environment: "production",
      maxBreadcrumbs: 50,
      release: "artisan-forge@1.2.3+abc",
      sendClientReports: false,
      sendDefaultPii: false,
      tracesSampleRate: 0,
    });
    expect(options.beforeBreadcrumb({ category: "console", message: "private" })).toBeNull();
    expect(
      options.beforeSend({
        exception: { values: [{ type: "Error", value: "secret C:\\Users\\sander" }] },
        request: { data: "private" },
      }),
    ).toMatchObject({ exception: { values: [{ type: "Error", value: "[SANITIZED]" }] } });
    expect(
      options.beforeSend({ extra: { nested: "ARTISAN_TELEMETRY_FORBIDDEN_CANARY" } }),
    ).toBeNull();
  });

  it("rechecks consent at send time and reports previous unclean exit without a false stack", () => {
    const current = store("enabled");
    const sdk = { captureMessage: vi.fn(), flush: vi.fn(), init: vi.fn() };
    const runtime = InitializeForgeSentry({
      dsn: "https://public@example.ingest.sentry.io/1",
      metadata,
      preferences: current.preferences,
      previous_exit: "unclean",
      sdk,
    });
    const options = sdk.init.mock.calls[0]![0];

    expect(sdk.captureMessage).toHaveBeenCalledWith("Forge previously exited uncleanly", {
      level: "error",
      tags: { artisan_code: "forge_previous_unclean_exit" },
    });
    current.preferences.update({ crash_reports: "disabled" });
    expect(options.beforeSend({ message: "anything" })).toBeNull();
    expect(runtime.capture("startup_failed")).toBeUndefined();
  });
});
