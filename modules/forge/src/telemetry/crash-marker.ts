import { chmodSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

interface ForgeCrashMarkerRecord {
  readonly clean: boolean;
  readonly commit: string;
  readonly heartbeat_at: string;
  readonly memory_bucket_mb: number;
  readonly release: string;
  readonly started_at: string;
  readonly version: 1;
}

export interface ForgeCrashMarkerInput {
  readonly commit: string;
  readonly marker_path: string;
  readonly release: string;
  readonly started_at: string;
}

export type PreviousForgeExit = "clean" | "unclean" | "unknown";

export interface ForgeCrashMarker {
  readonly previous_exit: PreviousForgeExit;
  readonly heartbeat: (input: { readonly at: string; readonly memory_bytes: number }) => void;
  readonly mark_clean: (at: string) => void;
}

const IsRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const DecodeMarker = (value: unknown): ForgeCrashMarkerRecord | undefined => {
  if (!IsRecord(value)) return undefined;
  if (
    value.version !== 1 ||
    typeof value.clean !== "boolean" ||
    typeof value.commit !== "string" ||
    typeof value.heartbeat_at !== "string" ||
    typeof value.memory_bucket_mb !== "number" ||
    !Number.isSafeInteger(value.memory_bucket_mb) ||
    value.memory_bucket_mb < 0 ||
    typeof value.release !== "string" ||
    typeof value.started_at !== "string"
  ) {
    return undefined;
  }
  return value as unknown as ForgeCrashMarkerRecord;
};

const ReadMarker = (path: string): ForgeCrashMarkerRecord | undefined => {
  try {
    return DecodeMarker(JSON.parse(readFileSync(path, "utf8")));
  } catch {
    return undefined;
  }
};

const WritePrivateMarker = (path: string, marker: ForgeCrashMarkerRecord) => {
  mkdirSync(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.tmp`;
  try {
    writeFileSync(temporary, `${JSON.stringify(marker)}\n`, { mode: 0o600 });
    chmodSync(temporary, 0o600);
    renameSync(temporary, path);
    chmodSync(path, 0o600);
  } finally {
    rmSync(temporary, { force: true });
  }
};

const MemoryBucketMb = (bytes: number): number => {
  if (!Number.isFinite(bytes) || bytes <= 0) return 0;
  const megabytes = Math.ceil(bytes / (1024 * 1024));
  let bucket = 64;
  while (bucket < megabytes && bucket < 32_768) bucket *= 2;
  return Math.min(bucket, 32_768);
};

/**
 * Starts a minimal local heartbeat record. It intentionally exposes only a
 * coarse previous-exit classification; callers cannot upload marker contents.
 */
export const StartForgeCrashMarker = (input: ForgeCrashMarkerInput): ForgeCrashMarker => {
  const previous = ReadMarker(input.marker_path);
  const previous_exit: PreviousForgeExit =
    previous === undefined ? "unknown" : previous.clean ? "clean" : "unclean";
  let current: ForgeCrashMarkerRecord = {
    clean: false,
    commit: input.commit,
    heartbeat_at: input.started_at,
    memory_bucket_mb: 0,
    release: input.release,
    started_at: input.started_at,
    version: 1,
  };
  WritePrivateMarker(input.marker_path, current);

  return {
    heartbeat: ({ at, memory_bytes }) => {
      current = {
        ...current,
        clean: false,
        heartbeat_at: at,
        memory_bucket_mb: MemoryBucketMb(memory_bytes),
      };
      WritePrivateMarker(input.marker_path, current);
    },
    mark_clean: (at) => {
      current = { ...current, clean: true, heartbeat_at: at };
      WritePrivateMarker(input.marker_path, current);
    },
    previous_exit,
  };
};
