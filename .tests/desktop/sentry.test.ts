import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import { InitializeDesktopSentry } from "../../modules/desktop/src/sentry";

const enabled_preferences = (path: string) =>
  writeFileSync(
    path,
    JSON.stringify({
      crash_reports: "enabled",
      installation_id: "0192a5f0-85f8-7abc-9def-0123456789ab",
      updated_at: "2026-08-22T00:00:00.000Z",
      usage_analytics: "disabled",
      version: 1,
    }),
  );

describe("desktop Sentry", () => {
  it("disables Electron defaults so minidumps, screenshots, and request integrations stay off", () => {
    const source = readFileSync(
      new URL("../../modules/desktop/src/main.ts", import.meta.url),
      "utf8",
    );
    expect(source).toContain("onUncaughtExceptionIntegration");
    expect(source).toContain("onUnhandledRejectionIntegration");
    expect(source).not.toContain("sentryMinidumpIntegration");
    expect(source).not.toContain("screenshotsIntegration");
  });
  it("does not initialize without explicit crash consent", () => {
    const sdk = { captureMessage: vi.fn(), flush: vi.fn(), init: vi.fn() };
    const runtime = InitializeDesktopSentry({
      config: {
        dsn: "https://public@example.ingest.sentry.io/1",
        environment: "production",
        release: "artisan-editor@1.2.3+abc",
      },
      preferences_path: undefined,
      sdk,
    });
    expect(runtime.enabled).toBe(false);
    expect(sdk.init).not.toHaveBeenCalled();
  });

  it("uses the shared privacy hook and never enables default breadcrumbs", () => {
    const root = mkdtempSync(join(tmpdir(), "artisan-desktop-sentry-"));
    const preferences_path = join(root, "telemetry.json");
    enabled_preferences(preferences_path);
    const sdk = { captureMessage: vi.fn(), flush: vi.fn(), init: vi.fn() };
    const runtime = InitializeDesktopSentry({
      config: {
        dsn: "https://public@example.ingest.sentry.io/1",
        environment: "production",
        release: "artisan-editor@1.2.3+abc",
      },
      preferences_path,
      sdk,
    });
    expect(runtime.enabled).toBe(true);
    const options = sdk.init.mock.calls[0]?.[0];
    expect(options.sendDefaultPii).toBe(false);
    expect(options.sendClientReports).toBe(false);
    expect(options.tracesSampleRate).toBe(0);
    expect(options.defaultIntegrations).toBe(false);
    expect(options.beforeBreadcrumb({ category: "console", message: "secret" })).toBeNull();
    expect(options.beforeSend({ message: "ARTISAN_TELEMETRY_FORBIDDEN_CANARY" })).toBeNull();
    runtime.capture("renderer_gone", { renderer_reason: "crashed" });
    expect(sdk.captureMessage).toHaveBeenCalledWith("Artisan renderer gone", {
      level: "error",
      tags: {
        artisan_code: "renderer_gone",
        renderer_reason: "crashed",
        runtime: "desktop_main",
      },
    });
  });
});
