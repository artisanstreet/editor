import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { StartForgeCrashMarker } from "../../modules/forge/src/telemetry/crash-marker";

const fixture = () => ({
  commit: "abc123",
  marker_path: join(mkdtempSync(join(tmpdir(), "artisan-crash-marker-")), "forge-crash.json"),
  release: "artisan-forge@1.2.3+abc123",
  started_at: "2026-08-22T12:00:00.000Z",
});

describe("Forge crash marker", () => {
  it("reports no previous exit on first launch and writes only coarse safe state", () => {
    const input = fixture();
    const marker = StartForgeCrashMarker(input);

    expect(marker.previous_exit).toBe("unknown");
    marker.heartbeat({ at: "2026-08-22T12:00:05.000Z", memory_bytes: 70 * 1024 * 1024 });

    const persisted = JSON.parse(readFileSync(input.marker_path, "utf8")) as Record<
      string,
      unknown
    >;
    expect(persisted).toEqual({
      clean: false,
      commit: "abc123",
      heartbeat_at: "2026-08-22T12:00:05.000Z",
      memory_bucket_mb: 128,
      release: "artisan-forge@1.2.3+abc123",
      started_at: "2026-08-22T12:00:00.000Z",
      version: 1,
    });
    expect(JSON.stringify(persisted)).not.toMatch(/path|prompt|terminal|token|command/iu);
  });

  it("detects one unclean previous exit after a forced-stop simulation", () => {
    const input = fixture();
    StartForgeCrashMarker(input).heartbeat({
      at: "2026-08-22T12:00:05.000Z",
      memory_bytes: 260 * 1024 * 1024,
    });

    const restarted = StartForgeCrashMarker({
      ...input,
      started_at: "2026-08-22T12:01:00.000Z",
    });
    expect(restarted.previous_exit).toBe("unclean");

    restarted.mark_clean("2026-08-22T12:02:00.000Z");
    const next = StartForgeCrashMarker({
      ...input,
      started_at: "2026-08-22T12:03:00.000Z",
    });
    expect(next.previous_exit).toBe("clean");
  });

  it("fails closed on malformed or future marker data", () => {
    const input = fixture();
    StartForgeCrashMarker(input);
    const malformed = `${readFileSync(input.marker_path, "utf8").slice(0, 10)}not-json`;
    writeFileSync(input.marker_path, malformed);

    const restarted = StartForgeCrashMarker({
      ...input,
      started_at: "2026-08-22T12:01:00.000Z",
    });
    expect(restarted.previous_exit).toBe("unknown");
  });
});
