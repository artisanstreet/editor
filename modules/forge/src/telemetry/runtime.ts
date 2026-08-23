import * as Sentry from "@sentry/node-core";

import {
  make_product_telemetry_layer,
  make_telemetry_preferences_control_layer,
  ProductTelemetryNoop,
  TelemetryPreferencesControlNoop,
} from "@artisan/backend";
import { DecodeProductTelemetryEvent, type ProductTelemetryEvent } from "@artisan/observability";

import { DecodePostHogHost, MakePostHogClientFactory } from "./posthog-client";
import { MakePostHogTelemetry } from "./posthog";
import { MakeTelemetryPreferencesStore } from "./preferences";
import { InitializeForgeSentry } from "./sentry";

declare const __ARTISAN_POSTHOG_PROJECT_KEY__: string;
declare const __ARTISAN_POSTHOG_HOST__: string;
declare const __ARTISAN_SENTRY_FORGE_DSN__: string;
declare const __ARTISAN_TELEMETRY_ENVIRONMENT__: string;

const BuildValue = (value: string | undefined) =>
  value === undefined || value.trim().length === 0 ? undefined : value.trim();

export const ForgeBuildTelemetryConfig = {
  environment: BuildValue(
    typeof __ARTISAN_TELEMETRY_ENVIRONMENT__ === "undefined"
      ? undefined
      : __ARTISAN_TELEMETRY_ENVIRONMENT__,
  ),
  posthog_host: BuildValue(
    typeof __ARTISAN_POSTHOG_HOST__ === "undefined" ? undefined : __ARTISAN_POSTHOG_HOST__,
  ),
  posthog_project_key: BuildValue(
    typeof __ARTISAN_POSTHOG_PROJECT_KEY__ === "undefined"
      ? undefined
      : __ARTISAN_POSTHOG_PROJECT_KEY__,
  ),
  sentry_dsn: BuildValue(
    typeof __ARTISAN_SENTRY_FORGE_DSN__ === "undefined" ? undefined : __ARTISAN_SENTRY_FORGE_DSN__,
  ),
} as const;

const DecodeEnvironment = (value: string | undefined) =>
  value === "production" || value === "staging" ? value : ("development" as const);
const DecodePlatform = () =>
  process.platform === "win32"
    ? ("windows" as const)
    : process.platform === "darwin"
      ? ("macos" as const)
      : ("linux" as const);
const DecodeArch = () =>
  process.arch === "x64"
    ? ("x64" as const)
    : process.arch === "arm64"
      ? ("arm64" as const)
      : ("other" as const);

const DecodeEvent = (input: unknown) => {
  try {
    return DecodeProductTelemetryEvent(input);
  } catch {
    return undefined;
  }
};

export interface ForgeTelemetryRuntime {
  readonly capture: (event: ProductTelemetryEvent, canonical_event_id: string) => Promise<void>;
  readonly crash_monitoring_enabled: boolean;
  readonly product_telemetry: typeof ProductTelemetryNoop;
  readonly telemetry_preferences: typeof TelemetryPreferencesControlNoop;
  readonly shutdown: () => Promise<void>;
}

export const MakeForgeTelemetryRuntime = (input: {
  readonly app_version: string;
  readonly crash_reports_dsn: string | undefined;
  readonly environment: string | undefined;
  readonly forge_mode: "headless" | "local";
  readonly posthog_host: string | undefined;
  readonly posthog_project_key: string | undefined;
  readonly previous_exit: "clean" | "unclean" | "unknown";
  readonly release: string;
  readonly release_channel: "beta" | "development" | "stable";
  readonly telemetry_config_path: string | undefined;
}): ForgeTelemetryRuntime => {
  if (input.telemetry_config_path === undefined) {
    return {
      capture: async () => undefined,
      crash_monitoring_enabled: false,
      product_telemetry: ProductTelemetryNoop,
      shutdown: async () => undefined,
      telemetry_preferences: TelemetryPreferencesControlNoop,
    };
  }
  const preferences = MakeTelemetryPreferencesStore(input.telemetry_config_path);
  const environment = DecodeEnvironment(input.environment);
  let posthog_project_key = input.posthog_project_key;
  let posthog_host = DecodePostHogHost(undefined);
  try {
    posthog_host = DecodePostHogHost(input.posthog_host);
  } catch {
    // A malformed release/runtime endpoint disables analytics; it must never prevent Forge startup.
    posthog_project_key = undefined;
  }
  const posthog = MakePostHogTelemetry({
    client_factory: MakePostHogClientFactory(posthog_host),
    decode_event: DecodeEvent,
    metadata: {
      app_version: input.app_version,
      arch: DecodeArch(),
      environment,
      forge_mode: input.forge_mode,
      is_packaged: environment !== "development",
      platform: DecodePlatform(),
      release: input.release,
      release_channel: input.release_channel,
      surface: "forge",
    },
    preferences,
    project_key: posthog_project_key,
  });
  const sentry = InitializeForgeSentry({
    dsn: input.crash_reports_dsn,
    metadata: {
      arch: DecodeArch(),
      environment,
      platform: DecodePlatform(),
      release: input.release,
      release_channel: input.release_channel,
      runtime: "forge",
    },
    preferences,
    previous_exit: input.previous_exit,
    sdk: {
      captureMessage: (message, context) => Sentry.captureMessage(message, context),
      flush: (timeout) => Sentry.flush(timeout),
      init: (options) =>
        Sentry.initWithoutDefaultIntegrations({
          ...options,
          integrations: [
            Sentry.onUncaughtExceptionIntegration(),
            Sentry.onUnhandledRejectionIntegration(),
          ],
        }),
    },
  });
  return {
    capture: posthog.capture,
    crash_monitoring_enabled: sentry.enabled,
    product_telemetry: make_product_telemetry_layer({ capture: posthog.capture }),
    shutdown: async () => {
      await posthog.shutdown(500);
      await sentry.flush();
    },
    telemetry_preferences: make_telemetry_preferences_control_layer({
      read: preferences.read_public,
      update: preferences.update,
    }),
  };
};
