import { model_manifest } from "@artisan/catalog";

export const ProductTelemetryEventNames = [
  "forge_started",
  "forge_stopped",
  "editor_session_started",
  "forge_connection_finished",
  "project_added",
  "project_opened",
  "thread_created",
  "run_started",
  "run_finished",
  "engine_setup_finished",
  "model_transition_finished",
  "tool_approval_resolved",
  "usage_interruption_detected",
  "usage_recovery_finished",
  "thread_retention_changed",
  "feature_used",
] as const;

export type ProductTelemetryEventName = (typeof ProductTelemetryEventNames)[number];
export type TelemetryProperty = boolean | number | string;
export interface ProductTelemetryEvent {
  readonly event: ProductTelemetryEventName;
  readonly properties: Readonly<Record<string, TelemetryProperty>>;
}

const event_names = new Set<string>(ProductTelemetryEventNames);
const catalog_models = new Set(model_manifest.models.map((model) => model.id));
const built_in_engines = new Set(["codex", "claude", "opencode2", "grok", "cursor", "hermes"]);
const failure_codes = new Set([
  "none",
  "authentication",
  "configuration",
  "network",
  "engine_unavailable",
  "timeout",
  "cancelled",
  "internal",
  "persistence",
  "protocol",
  "unknown",
]);

export const NormalizeCatalogModelId = (model_id: string | undefined): string =>
  model_id !== undefined && catalog_models.has(model_id) ? model_id : "custom_or_unknown";

const IsRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);
const Enum = (values: ReadonlyArray<string>) => {
  const allowed = new Set(values);
  return (value: unknown) => typeof value === "string" && allowed.has(value);
};
const BooleanValue = (value: unknown) => typeof value === "boolean";
const BoundedInteger = (maximum: number) => (value: unknown) =>
  typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
const Duration = BoundedInteger(604_800_000);
const Count = BoundedInteger(1_000_000_000);
const Engine = (value: unknown) => typeof value === "string" && built_in_engines.has(value);
const Model = (value: unknown) =>
  typeof value === "string" && (value === "custom_or_unknown" || catalog_models.has(value));
const FailureCode = (value: unknown) => typeof value === "string" && failure_codes.has(value);

type Validator = (value: unknown) => boolean;
interface EventSpec {
  readonly optional?: Readonly<Record<string, Validator>>;
  readonly required: Readonly<Record<string, Validator>>;
}

const specs: Readonly<Record<ProductTelemetryEventName, EventSpec>> = {
  forge_started: {
    required: {
      cold_start_duration_ms: Duration,
      forge_mode: Enum(["local", "headless"]),
      previous_exit: Enum(["clean", "unclean", "unknown"]),
    },
  },
  forge_stopped: {
    required: {
      shutdown_reason: Enum(["requested", "parent_disconnect", "signal", "update"]),
      uptime_ms: Duration,
    },
  },
  editor_session_started: {
    required: {
      forge_connection: Enum(["local", "remote"]),
      surface: Enum(["desktop_renderer", "browser_renderer"]),
      time_to_ready_ms: Duration,
    },
  },
  forge_connection_finished: {
    optional: { failure_code: FailureCode },
    required: {
      attempt: Enum(["initial", "reconnect", "resume"]),
      duration_ms: Duration,
      outcome: Enum(["connected", "failed"]),
    },
  },
  project_added: {
    required: {
      is_first_project: BooleanValue,
      kind: Enum(["git", "directory"]),
      source: Enum(["picker", "explicit"]),
    },
  },
  project_opened: {
    required: { is_first_open: BooleanValue, kind: Enum(["git", "directory"]) },
  },
  thread_created: {
    optional: { is_first_thread: BooleanValue },
    required: {
      engine_id: Engine,
      has_image_attachment: BooleanValue,
      model_id: Model,
      permission: Enum(["standard", "auto", "plan", "full"]),
    },
  },
  run_started: {
    required: {
      continuation_kind: Enum(["new", "continue", "retry", "transition"]),
      engine_id: Engine,
      has_image_attachment: BooleanValue,
      model_id: Model,
      permission: Enum(["standard", "auto", "plan", "full"]),
    },
  },
  run_finished: {
    optional: {
      cache_tokens: Count,
      failure_code: FailureCode,
      input_tokens: Count,
      output_tokens: Count,
    },
    required: {
      duration_ms: Duration,
      engine_id: Engine,
      model_id: Model,
      outcome: Enum(["completed", "failed", "cancelled"]),
      permission: Enum(["standard", "auto", "plan", "full"]),
    },
  },
  engine_setup_finished: {
    optional: { failure_code: FailureCode },
    required: {
      engine_id: Engine,
      operation: Enum(["install", "update", "authenticate"]),
      outcome: Enum(["completed", "failed", "cancelled"]),
    },
  },
  model_transition_finished: {
    optional: { failure_code: FailureCode },
    required: {
      continuation_strategy: Enum(["native", "summary", "fresh"]),
      outcome: Enum(["completed", "failed", "cancelled"]),
      source_engine_id: Engine,
      source_model_id: Model,
      target_engine_id: Engine,
      target_model_id: Model,
    },
  },
  tool_approval_resolved: {
    required: {
      decision: Enum(["approved_once", "approved_session", "denied"]),
      tool_category: Enum([
        "file_read",
        "file_write",
        "shell",
        "search",
        "browser",
        "integration",
        "other",
      ]),
      wait_duration_ms: Duration,
    },
  },
  usage_interruption_detected: {
    required: {
      engine_id: Engine,
      interruption_kind: Enum(["usage_limit", "authentication", "network", "unknown"]),
      model_id: Model,
    },
  },
  usage_recovery_finished: {
    required: {
      mode: Enum(["manual", "automatic"]),
      outcome: Enum(["completed", "failed", "cancelled"]),
      wait_duration_ms: Duration,
    },
  },
  thread_retention_changed: {
    required: { retention: Enum(["forever", "30_days", "90_days", "1_year"]) },
  },
  feature_used: {
    required: {
      feature: Enum([
        "subagent_graph",
        "checkpoint_rollback",
        "workspace_review",
        "routine",
        "capability",
        "preview",
        "thread_search",
      ]),
    },
  },
};

/** Strict runtime boundary for every event before it reaches a vendor queue. */
export const DecodeProductTelemetryEvent = (input: unknown): ProductTelemetryEvent => {
  if (
    !IsRecord(input) ||
    Object.keys(input).length !== 2 ||
    !event_names.has(String(input.event))
  ) {
    throw new Error("Unknown product telemetry event");
  }
  if (!IsRecord(input.properties)) throw new Error("Telemetry properties must be an object");
  const event = input.event as ProductTelemetryEventName;
  const spec = specs[event];
  const allowed = new Set([...Object.keys(spec.required), ...Object.keys(spec.optional ?? {})]);
  const keys = Object.keys(input.properties);
  if (keys.some((key) => !allowed.has(key))) throw new Error("Unknown telemetry property");
  for (const [key, validator] of Object.entries(spec.required)) {
    if (!(key in input.properties) || !validator(input.properties[key])) {
      throw new Error(`Invalid required telemetry property: ${key}`);
    }
  }
  for (const [key, validator] of Object.entries(spec.optional ?? {})) {
    if (key in input.properties && !validator(input.properties[key])) {
      throw new Error(`Invalid optional telemetry property: ${key}`);
    }
  }
  return { event, properties: { ...(input.properties as Record<string, TelemetryProperty>) } };
};
