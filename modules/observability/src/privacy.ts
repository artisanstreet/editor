export const TelemetryForbiddenCanary = "ARTISAN_TELEMETRY_FORBIDDEN_CANARY";
const maximum_string_length = 512;
const maximum_depth = 8;
const maximum_entries = 100;

const bearer = /\bBearer\s+[^\s,;]+/giu;
const credential = /\b(?:sk|pk|phc|phx|phs|api|token|secret)[-_][A-Za-z0-9_-]{12,}\b/giu;
const email = /\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b/giu;
const url = /\b(?:https?|wss?|postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis):\/\/[^\s]+/giu;
const windows_path = /\b[A-Z]:[\\/][^\s,;"']+/giu;
const posix_path = /(?:^|\s)\/(?:Users|home|var|tmp|etc|opt|mnt|srv|workspace)\/[^\s,;"']+/gu;

export const SanitizeTelemetryText = (input: string): string =>
  input
    .slice(0, maximum_string_length)
    .replace(bearer, "[REDACTED]")
    .replace(credential, "[REDACTED]")
    .replace(url, "[REDACTED]")
    .replace(email, "[REDACTED]")
    .replace(windows_path, "[REDACTED]")
    .replace(posix_path, " [REDACTED]");

export const ContainsTelemetryCanary = (input: unknown, depth = 0): boolean => {
  if (depth > maximum_depth) return false;
  if (typeof input === "string") return input.includes(TelemetryForbiddenCanary);
  if (Array.isArray(input)) {
    return input
      .slice(0, maximum_entries)
      .some((value) => ContainsTelemetryCanary(value, depth + 1));
  }
  if (typeof input === "object" && input !== null) {
    return Object.entries(input)
      .slice(0, maximum_entries)
      .some(
        ([key, value]) =>
          key.includes(TelemetryForbiddenCanary) || ContainsTelemetryCanary(value, depth + 1),
      );
  }
  return false;
};

const IsRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const tag_keys = new Set([
  "arch",
  "artisan_code",
  "memory_bucket",
  "operation",
  "platform",
  "release_channel",
  "renderer_reason",
  "runtime",
  "surface",
]);
const breadcrumb_data_keys = new Set([
  "artisan_code",
  "memory_bucket",
  "operation",
  "outcome",
  "reason",
  "runtime",
]);
const sentry_exception_types = new Set([
  "AggregateError",
  "Error",
  "EvalError",
  "RangeError",
  "ReferenceError",
  "SyntaxError",
  "SystemError",
  "TypeError",
  "URIError",
]);
const stable_code = /^[a-z][a-z0-9_.-]{0,63}$/u;
const field_values: Readonly<Record<string, ReadonlySet<string>>> = {
  arch: new Set(["arm64", "ia32", "x64"]),
  outcome: new Set(["cancelled", "completed", "connected", "failed", "succeeded"]),
  platform: new Set(["linux", "macos", "windows"]),
  reason: new Set(["error", "parent_disconnect", "requested", "signal", "unknown", "update"]),
  release_channel: new Set(["beta", "development", "stable"]),
  renderer_reason: new Set([
    "abnormal-exit",
    "clean-exit",
    "crashed",
    "integrity-failure",
    "killed",
    "launch-failed",
    "oom",
  ]),
  runtime: new Set([
    "browser_renderer",
    "desktop_main",
    "desktop_renderer",
    "electron_main",
    "forge",
  ]),
  surface: new Set(["browser_renderer", "desktop_main", "desktop_renderer", "forge"]),
};
const StablePrimitive = (key: string, value: unknown): boolean | number | string | undefined => {
  if (typeof value === "boolean" || typeof value === "number") return value;
  if (typeof value !== "string") return undefined;
  const allowed = field_values[key];
  if (allowed !== undefined) return allowed.has(value) ? value : undefined;
  return stable_code.test(value) ? value : undefined;
};

export const SanitizeSentryBreadcrumb = (input: unknown): Record<string, unknown> | null => {
  if (
    !IsRecord(input) ||
    input.category !== "artisan.lifecycle" ||
    ContainsTelemetryCanary(input)
  ) {
    return null;
  }
  const output: Record<string, unknown> = { category: "artisan.lifecycle" };
  if (
    typeof input.level === "string" &&
    new Set(["debug", "error", "fatal", "info", "warning"]).has(input.level)
  )
    output.level = input.level;
  if (typeof input.timestamp === "number") output.timestamp = input.timestamp;
  if (IsRecord(input.data)) {
    const data: Record<string, boolean | number | string> = {};
    for (const [key, value] of Object.entries(input.data).slice(0, maximum_entries)) {
      if (!breadcrumb_data_keys.has(key)) continue;
      const safe = StablePrimitive(key, value);
      if (safe !== undefined) data[key] = safe;
    }
    if (Object.keys(data).length > 0) output.data = data;
  }
  return output;
};

const SafeFramePath = (input: unknown): string | undefined => {
  if (typeof input !== "string") return undefined;
  const normalized = input.replaceAll("\\", "/").split(/[?#]/u, 1)[0] ?? "";
  const renderer_asset = normalized.match(
    /\/_app\/immutable\/(?:chunks\/[A-Za-z0-9_-]{8,12}|nodes\/[0-9]{1,3}\.[A-Za-z0-9_-]{8,12}|entry\/(?:app|start)\.[A-Za-z0-9_-]{8,12})\.js$/u,
  )?.[0];
  if (renderer_asset !== undefined) return `app:///${renderer_asset.slice(1)}`;
  if (normalized.endsWith("/main.js") || normalized === "main.js") return "app:///main.js";
  if (normalized.endsWith("/forge-main.cjs") || normalized === "forge-main.cjs") {
    return "app:///forge-main.cjs";
  }
  return undefined;
};

const debug_id = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu;
const SanitizeDebugMeta = (input: unknown): Record<string, unknown> | undefined => {
  if (!IsRecord(input) || !Array.isArray(input.images)) return undefined;
  const images = input.images.slice(0, 32).flatMap((candidate) => {
    if (
      !IsRecord(candidate) ||
      candidate.type !== "sourcemap" ||
      typeof candidate.debug_id !== "string" ||
      !debug_id.test(candidate.debug_id)
    ) {
      return [];
    }
    const code_file = SafeFramePath(candidate.code_file);
    if (code_file === undefined) return [];
    return [
      {
        code_file,
        debug_id: candidate.debug_id.toLowerCase(),
        type: "sourcemap",
      },
    ];
  });
  return images.length > 0 ? { images } : undefined;
};

const SanitizeFrame = (input: unknown): Record<string, unknown> | undefined => {
  if (!IsRecord(input)) return undefined;
  const path = SafeFramePath(input.abs_path ?? input.filename);
  const output: Record<string, unknown> = {};
  if (path !== undefined) {
    output.abs_path = path;
    output.filename = path;
  }
  // Function and module names can come directly from user-authored source code.
  // Source maps need only the generated frame path and coordinates.
  if (typeof input.lineno === "number") output.lineno = input.lineno;
  if (typeof input.colno === "number") output.colno = input.colno;
  if (typeof input.in_app === "boolean") output.in_app = input.in_app;
  return Object.keys(output).length === 0 ? undefined : output;
};

const SanitizeException = (input: unknown): Record<string, unknown> | undefined => {
  if (!IsRecord(input) || !Array.isArray(input.values)) return undefined;
  const values = input.values.slice(0, 8).flatMap((candidate) => {
    if (!IsRecord(candidate)) return [];
    const value: Record<string, unknown> = { value: "[SANITIZED]" };
    value.type =
      typeof candidate.type === "string" && sentry_exception_types.has(candidate.type)
        ? candidate.type
        : "Error";
    if (IsRecord(candidate.stacktrace) && Array.isArray(candidate.stacktrace.frames)) {
      const frames = candidate.stacktrace.frames
        .slice(-100)
        .map(SanitizeFrame)
        .filter((frame): frame is Record<string, unknown> => frame !== undefined);
      if (frames.length > 0) value.stacktrace = { frames };
    }
    return [value];
  });
  return values.length > 0 ? { values } : undefined;
};

/**
 * Deny-by-default Sentry boundary shared by browser, Electron main, and Forge.
 * Unknown containers are discarded rather than recursively forwarded.
 */
export const SanitizeSentryEvent = (input: unknown): Record<string, unknown> | null => {
  if (!IsRecord(input) || ContainsTelemetryCanary(input)) return null;
  const output: Record<string, unknown> = {};
  for (const key of ["event_id", "environment", "level", "platform", "release", "dist"] as const) {
    if (typeof input[key] === "string") output[key] = SanitizeTelemetryText(input[key]);
  }
  if (typeof input.timestamp === "number") output.timestamp = input.timestamp;
  if (typeof input.message === "string") output.message = "[SANITIZED]";
  const debug_meta = SanitizeDebugMeta(input.debug_meta);
  if (debug_meta !== undefined) output.debug_meta = debug_meta;
  const exception = SanitizeException(input.exception);
  if (exception !== undefined) output.exception = exception;
  if (IsRecord(input.tags)) {
    const tags: Record<string, boolean | number | string> = {};
    for (const [key, value] of Object.entries(input.tags).slice(0, maximum_entries)) {
      if (!tag_keys.has(key)) continue;
      const safe = StablePrimitive(key, value);
      if (safe !== undefined) tags[key] = safe;
    }
    if (Object.keys(tags).length > 0) output.tags = tags;
  }
  if (Array.isArray(input.breadcrumbs)) {
    const breadcrumbs = input.breadcrumbs
      .slice(-50)
      .map(SanitizeSentryBreadcrumb)
      .filter((breadcrumb): breadcrumb is Record<string, unknown> => breadcrumb !== null);
    if (breadcrumbs.length > 0) output.breadcrumbs = breadcrumbs;
  }
  return output;
};
