import { SanitizeSentryBreadcrumb, SanitizeSentryEvent } from "@artisan/observability";

import type { TelemetryPreferencesStore } from "./preferences";

export interface ForgeSentrySdkPort {
  readonly captureMessage: (
    message: string,
    context?: {
      readonly level: "error" | "fatal" | "warning";
      readonly tags: Record<string, string>;
    },
  ) => unknown;
  readonly flush: (timeout?: number) => Promise<boolean> | boolean;
  readonly init: (options: Record<string, unknown>) => unknown;
}

export interface ForgeSentryMetadata {
  readonly arch: "arm64" | "other" | "x64";
  readonly environment: "development" | "production" | "staging" | "test";
  readonly platform: "linux" | "macos" | "windows";
  readonly release: string;
  readonly release_channel: "beta" | "development" | "stable";
  readonly runtime: "forge";
}

const capture_codes = new Set([
  "host_failed",
  "job_failed",
  "memory_critical",
  "request_failed",
  "startup_failed",
]);

const IsCrashReportingEnabled = (preferences: TelemetryPreferencesStore) =>
  preferences.read_for_runtime().crash_reports === "enabled";

export const InitializeForgeSentry = (input: {
  readonly dsn: string | undefined;
  readonly metadata: ForgeSentryMetadata;
  readonly preferences: TelemetryPreferencesStore;
  readonly previous_exit?: "clean" | "unclean" | "unknown";
  readonly sdk: ForgeSentrySdkPort;
}) => {
  const enabled =
    input.dsn !== undefined &&
    (input.metadata.environment === "production" || input.metadata.environment === "staging") &&
    IsCrashReportingEnabled(input.preferences);
  if (!enabled) {
    return {
      capture: (_code: string) => undefined,
      enabled: false as const,
      flush: async () => false,
    };
  }

  input.sdk.init({
    attachStacktrace: true,
    beforeBreadcrumb: (breadcrumb: unknown) => SanitizeSentryBreadcrumb(breadcrumb),
    beforeSend: (event: unknown) =>
      IsCrashReportingEnabled(input.preferences) ? SanitizeSentryEvent(event) : null,
    beforeSendTransaction: () => null,
    dsn: input.dsn,
    enableLogs: false,
    environment: input.metadata.environment,
    initialScope: {
      tags: {
        arch: input.metadata.arch,
        platform: input.metadata.platform,
        release_channel: input.metadata.release_channel,
        runtime: input.metadata.runtime,
      },
    },
    maxBreadcrumbs: 50,
    release: input.metadata.release,
    sendClientReports: false,
    sendDefaultPii: false,
    tracesSampleRate: 0,
  });

  if (input.previous_exit === "unclean") {
    input.sdk.captureMessage("Forge previously exited uncleanly", {
      level: "error",
      tags: { artisan_code: "forge_previous_unclean_exit" },
    });
  }

  return {
    capture: (code: string) => {
      if (!capture_codes.has(code) || !IsCrashReportingEnabled(input.preferences)) return;
      input.sdk.captureMessage("Forge internal failure", {
        level: "error",
        tags: { artisan_code: code },
      });
    },
    enabled: true as const,
    flush: async () => {
      if (!IsCrashReportingEnabled(input.preferences)) return false;
      try {
        return await input.sdk.flush(500);
      } catch {
        return false;
      }
    },
  };
};
