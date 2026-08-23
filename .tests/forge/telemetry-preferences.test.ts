import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { MakeTelemetryPreferencesStore } from "../../modules/forge/src/telemetry/preferences";

const fixture = () => {
  const path = join(mkdtempSync(join(tmpdir(), "artisan-telemetry-prefs-")), "telemetry.json");
  writeFileSync(
    path,
    JSON.stringify({
      crash_reports: "unset",
      installation_id: "123e4567-e89b-42d3-a456-426614174000",
      updated_at: "2026-08-22T12:00:00.000Z",
      usage_analytics: "unset",
      version: 1,
    }),
  );
  return path;
};

describe("Forge telemetry preferences", () => {
  it("exposes public consent without exposing installation identity", () => {
    const store = MakeTelemetryPreferencesStore(fixture());

    expect(store.read_public()).toEqual({
      crash_reports: "unset",
      usage_analytics: "unset",
      version: 1,
    });
    expect(store.read_public()).not.toHaveProperty("installation_id");
  });

  it("updates choices independently with an atomic private replacement", () => {
    const path = fixture();
    const store = MakeTelemetryPreferencesStore(path, {
      now: () => "2026-08-22T12:05:00.000Z",
    });

    expect(store.update({ usage_analytics: "enabled" })).toEqual({
      crash_reports: "unset",
      usage_analytics: "enabled",
      version: 1,
    });
    const persisted = JSON.parse(readFileSync(path, "utf8"));
    expect(persisted).toMatchObject({
      crash_reports: "unset",
      installation_id: "123e4567-e89b-42d3-a456-426614174000",
      updated_at: "2026-08-22T12:05:00.000Z",
      usage_analytics: "enabled",
    });
  });

  it("fails closed when the file is missing, malformed, or has a future version", () => {
    const missing = MakeTelemetryPreferencesStore(join(tmpdir(), `missing-${Date.now()}.json`));
    expect(missing.read_for_runtime()).toEqual({
      crash_reports: "disabled",
      installation_id: undefined,
      usage_analytics: "disabled",
    });

    const malformed_path = fixture();
    writeFileSync(malformed_path, "not json");
    expect(MakeTelemetryPreferencesStore(malformed_path).read_for_runtime()).toMatchObject({
      crash_reports: "disabled",
      usage_analytics: "disabled",
    });

    const future_path = fixture();
    const future = JSON.parse(readFileSync(future_path, "utf8"));
    writeFileSync(future_path, JSON.stringify({ ...future, version: 2 }));
    expect(MakeTelemetryPreferencesStore(future_path).read_for_runtime()).toMatchObject({
      crash_reports: "disabled",
      usage_analytics: "disabled",
    });
  });

  it("rejects empty, unknown, and content-bearing updates", () => {
    const store = MakeTelemetryPreferencesStore(fixture());
    for (const patch of [
      {},
      { usage_analytics: "sometimes" },
      { usage_analytics: "enabled", prompt: "private" },
    ]) {
      expect(() => store.update(patch as never)).toThrow();
    }
  });
});
