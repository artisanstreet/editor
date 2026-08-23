import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

import type {
  TelemetryPreference,
  TelemetryPreferences,
  TelemetryPreferencesUpdate,
} from "@artisan/protocol";

interface StoredTelemetryPreferences extends TelemetryPreferences {
  readonly installation_id: string;
  readonly updated_at: string;
}

interface RuntimeTelemetryPreferences {
  readonly crash_reports: TelemetryPreference;
  readonly installation_id: string | undefined;
  readonly usage_analytics: TelemetryPreference;
}

export interface TelemetryPreferencesStore {
  readonly read_for_runtime: () => RuntimeTelemetryPreferences;
  readonly read_public: () => TelemetryPreferences;
  readonly update: (patch: TelemetryPreferencesUpdate) => TelemetryPreferences;
}

interface TelemetryPreferencesStoreOptions {
  readonly now?: () => string;
}

const choices = new Set<TelemetryPreference>(["unset", "enabled", "disabled"]);
const stored_keys = new Set([
  "crash_reports",
  "installation_id",
  "updated_at",
  "usage_analytics",
  "version",
]);
const update_keys = new Set(["crash_reports", "usage_analytics"]);
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;

const IsRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const DecodeStored = (value: unknown): StoredTelemetryPreferences => {
  if (!IsRecord(value) || Object.keys(value).some((key) => !stored_keys.has(key))) {
    throw new Error("Invalid telemetry preferences");
  }
  if (
    value.version !== 1 ||
    typeof value.installation_id !== "string" ||
    !uuid.test(value.installation_id) ||
    typeof value.updated_at !== "string" ||
    !choices.has(value.usage_analytics as TelemetryPreference) ||
    !choices.has(value.crash_reports as TelemetryPreference)
  ) {
    throw new Error("Invalid telemetry preferences");
  }
  return value as unknown as StoredTelemetryPreferences;
};

const DecodeUpdate = (value: unknown): TelemetryPreferencesUpdate => {
  if (!IsRecord(value)) throw new Error("Invalid telemetry preference update");
  const keys = Object.keys(value);
  if (keys.length === 0 || keys.some((key) => !update_keys.has(key))) {
    throw new Error("Invalid telemetry preference update");
  }
  for (const key of keys) {
    if (!choices.has(value[key] as TelemetryPreference)) {
      throw new Error("Invalid telemetry preference update");
    }
  }
  return value as TelemetryPreferencesUpdate;
};

const PublicPreferences = (stored: StoredTelemetryPreferences): TelemetryPreferences => ({
  crash_reports: stored.crash_reports,
  usage_analytics: stored.usage_analytics,
  version: 1,
});

const WritePrivateJson = (path: string, value: StoredTelemetryPreferences) => {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  try {
    writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
    chmodSync(temporary, 0o600);
    renameSync(temporary, path);
    chmodSync(path, 0o600);
  } finally {
    rmSync(temporary, { force: true });
  }
};

/** Reads and updates the CLI-owned home telemetry file without exposing identity over RPC. */
export const MakeTelemetryPreferencesStore = (
  path: string,
  options: TelemetryPreferencesStoreOptions = {},
): TelemetryPreferencesStore => {
  const Read = () => DecodeStored(JSON.parse(readFileSync(path, "utf8")));
  const now = options.now ?? (() => new Date().toISOString());

  return {
    read_for_runtime: () => {
      try {
        const preferences = Read();
        return {
          crash_reports: preferences.crash_reports,
          installation_id: preferences.installation_id,
          usage_analytics: preferences.usage_analytics,
        };
      } catch {
        return {
          crash_reports: "disabled",
          installation_id: undefined,
          usage_analytics: "disabled",
        };
      }
    },
    read_public: () => PublicPreferences(Read()),
    update: (input) => {
      const patch = DecodeUpdate(input);
      const current = Read();
      const updated: StoredTelemetryPreferences = {
        ...current,
        crash_reports: patch.crash_reports ?? current.crash_reports,
        usage_analytics: patch.usage_analytics ?? current.usage_analytics,
        updated_at: now(),
      };
      WritePrivateJson(path, updated);
      return PublicPreferences(updated);
    },
  };
};
