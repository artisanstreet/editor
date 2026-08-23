import { createHash } from "node:crypto";

import type { TelemetryPreferencesStore } from "./preferences";

export interface SafePostHogEvent {
  readonly event: string;
  readonly properties: Readonly<Record<string, boolean | number | string>>;
}

export interface PostHogClientPort {
  readonly capture: (input: {
    readonly distinctId: string;
    readonly event: string;
    readonly properties: Readonly<Record<string, boolean | number | string>>;
  }) => Promise<unknown> | unknown;
  readonly disable?: () => Promise<void>;
  readonly shutdown: () => Promise<unknown>;
}

export interface ProductTelemetryMetadata {
  readonly app_version: string;
  readonly arch: "arm64" | "x64" | "other";
  readonly environment: "production" | "staging" | "development" | "test";
  readonly forge_mode: "local" | "headless";
  readonly is_packaged: boolean;
  readonly platform: "windows" | "macos" | "linux" | "other";
  readonly release: string;
  readonly release_channel: "stable" | "beta" | "canary" | "development";
  readonly surface: "forge";
}

export interface PostHogTelemetry {
  readonly capture: (event: unknown, canonical_event_id: string) => Promise<void>;
  readonly shutdown: (deadline_ms?: number) => Promise<void>;
}

interface PostHogTelemetryOptions {
  readonly client_factory: (project_key: string) => PostHogClientPort;
  readonly decode_event: (input: unknown) => SafePostHogEvent | undefined;
  readonly metadata: ProductTelemetryMetadata;
  readonly preferences: Pick<TelemetryPreferencesStore, "read_for_runtime">;
  readonly project_key: string | undefined;
}

const IsRemoteEnvironment = (environment: ProductTelemetryMetadata["environment"]) =>
  environment === "production" || environment === "staging";

/** Consent-gated, failure-isolated PostHog port. Product code never sees the vendor SDK. */
export const MakePostHogTelemetry = (options: PostHogTelemetryOptions): PostHogTelemetry => {
  let client: PostHogClientPort | undefined;
  const Client = () => {
    if (client !== undefined) return client;
    if (options.project_key === undefined || options.project_key.length === 0) return undefined;
    client = options.client_factory(options.project_key);
    return client;
  };

  return {
    capture: async (input, canonical_event_id) => {
      try {
        if (!IsRemoteEnvironment(options.metadata.environment)) return;
        const preferences = options.preferences.read_for_runtime();
        if (
          preferences.usage_analytics !== "enabled" ||
          preferences.installation_id === undefined
        ) {
          const retiring = client;
          client = undefined;
          await retiring?.disable?.().catch(() => undefined);
          return;
        }
        const event = options.decode_event(input);
        if (event === undefined) return;
        const vendor = Client();
        if (vendor === undefined) return;
        const insert_id = createHash("sha256")
          .update(`${preferences.installation_id}\0${canonical_event_id}`)
          .digest("hex");
        await vendor.capture({
          distinctId: `install_${preferences.installation_id}`,
          event: event.event,
          properties: {
            ...options.metadata,
            ...event.properties,
            $geoip_disable: true,
            $insert_id: insert_id,
            event_schema_version: 1,
          },
        });
      } catch {
        // Analytics is never allowed to affect a product operation.
      }
    },
    shutdown: async (deadline_ms = 250) => {
      if (client === undefined) return;
      try {
        await Promise.race([
          client.shutdown(),
          new Promise<void>((resolve) => setTimeout(resolve, Math.max(0, deadline_ms))),
        ]);
      } catch {
        // Shutdown flushing is best effort and bounded.
      }
    },
  };
};
