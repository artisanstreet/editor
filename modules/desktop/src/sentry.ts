import { readFileSync } from "node:fs";

import { SanitizeSentryEvent } from "@artisan/observability";

export type DesktopSentryCode =
  | "bootstrap_failed"
  | "forge_recovery_failed"
  | "renderer_gone"
  | "renderer_unresponsive";

export interface DesktopSentrySdkPort {
  readonly captureMessage: (
    message: string,
    context: {
      readonly level: "error" | "warning";
      readonly tags: Readonly<Record<string, string>>;
    },
  ) => void;
  readonly flush: (timeout: number) => Promise<boolean>;
  readonly init: (options: Record<string, unknown>) => void;
}

interface DesktopSentryConfig {
  readonly dsn: string | undefined;
  readonly environment: string;
  readonly release: string;
}

const ChoiceEnabled = (path: string | undefined): boolean => {
  if (path === undefined) return false;
  try {
    const decoded: unknown = JSON.parse(readFileSync(path, "utf8"));
    if (typeof decoded !== "object" || decoded === null || Array.isArray(decoded)) return false;
    const record = decoded as Record<string, unknown>;
    return record.version === 1 && record.crash_reports === "enabled";
  } catch {
    return false;
  }
};

const ValidDsn = (input: string | undefined) => {
  if (input === undefined || input.trim() === "") return false;
  try {
    return new URL(input).protocol === "https:";
  } catch {
    return false;
  }
};

const message_for: Record<DesktopSentryCode, string> = {
  bootstrap_failed: "Artisan desktop bootstrap failed",
  forge_recovery_failed: "Artisan Forge recovery failed",
  renderer_gone: "Artisan renderer gone",
  renderer_unresponsive: "Artisan renderer unresponsive",
};

export const InitializeDesktopSentry = (input: {
  readonly config: DesktopSentryConfig;
  readonly preferences_path: string | undefined;
  readonly sdk: DesktopSentrySdkPort;
}) => {
  const enabled =
    ChoiceEnabled(input.preferences_path) &&
    ValidDsn(input.config.dsn) &&
    (input.config.environment === "production" || input.config.environment === "staging");
  if (!enabled) {
    return {
      capture: (_code: DesktopSentryCode, _tags: Readonly<Record<string, string>>) => {},
      enabled: false,
      flush: async () => false,
    };
  }
  input.sdk.init({
    autoSessionTracking: false,
    beforeBreadcrumb: () => null,
    beforeSend: (event: unknown) =>
      ChoiceEnabled(input.preferences_path) ? SanitizeSentryEvent(event) : null,
    defaultIntegrations: false,
    dsn: input.config.dsn,
    environment: input.config.environment,
    maxBreadcrumbs: 0,
    release: input.config.release,
    sendClientReports: false,
    sendDefaultPii: false,
    tracesSampleRate: 0,
  });
  return {
    capture: (code: DesktopSentryCode, tags: Readonly<Record<string, string>>) => {
      if (!ChoiceEnabled(input.preferences_path)) return;
      input.sdk.captureMessage(message_for[code], {
        level: code === "renderer_unresponsive" ? "warning" : "error",
        tags: { artisan_code: code, ...tags, runtime: "desktop_main" },
      });
    },
    enabled: true,
    flush: async () =>
      ChoiceEnabled(input.preferences_path) ? input.sdk.flush(500).catch(() => false) : false,
  };
};
