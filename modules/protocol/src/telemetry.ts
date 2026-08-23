import { Schema } from "effect";

/** Consent choices retain `unset` so no network client starts before an explicit choice. */
export const TelemetryPreference = Schema.Literals(["unset", "enabled", "disabled"]);
export type TelemetryPreference = typeof TelemetryPreference.Type;

/** Current independent product-analytics and crash-report choices. */
export const TelemetryPreferences = Schema.Struct({
  crash_reports: TelemetryPreference,
  usage_analytics: TelemetryPreference,
  version: Schema.Literal(1),
});
export type TelemetryPreferences = typeof TelemetryPreferences.Type;

/** A preferences patch must change at least one of the two closed settings. */
export const TelemetryPreferencesUpdate = Schema.Struct({
  crash_reports: Schema.optional(TelemetryPreference),
  usage_analytics: Schema.optional(TelemetryPreference),
}).check(
  Schema.makeFilter<{
    readonly crash_reports?: TelemetryPreference | undefined;
    readonly usage_analytics?: TelemetryPreference | undefined;
  }>((input) =>
    input.crash_reports === undefined && input.usage_analytics === undefined
      ? "Expected at least one telemetry preference"
      : undefined,
  ),
);
export type TelemetryPreferencesUpdate = typeof TelemetryPreferencesUpdate.Type;

/** Strictly decodes an untrusted telemetry-preference patch. */
export const DecodeTelemetryPreferencesUpdate = Schema.decodeUnknownEffect(
  TelemetryPreferencesUpdate,
  { onExcessProperty: "error" },
);

/** Renderer surfaces allowed to originate product-telemetry intents. */
export const RendererTelemetrySurface = Schema.Literals(["desktop_renderer", "browser_renderer"]);

const RendererDurationMs = Schema.Int.check(
  Schema.isGreaterThanOrEqualTo(0),
  Schema.isLessThanOrEqualTo(600_000),
);

/** Records one renderer session after the UI reaches its ready state. */
export const EditorSessionStartedTelemetryIntent = Schema.Struct({
  event: Schema.Literal("editor_session_started"),
  forge_connection: Schema.Literals(["local", "remote"]),
  surface: RendererTelemetrySurface,
  time_to_ready_ms: RendererDurationMs,
});

/** Records a successful renderer-to-Forge connection attempt. */
export const ForgeConnectionSucceededTelemetryIntent = Schema.Struct({
  attempt: Schema.Literals(["initial", "reconnect", "resume"]),
  duration_ms: RendererDurationMs,
  event: Schema.Literal("forge_connection_finished"),
  outcome: Schema.Literal("connected"),
});

/** Records a classified renderer-to-Forge connection failure without endpoint or error text. */
export const ForgeConnectionFailedTelemetryIntent = Schema.Struct({
  attempt: Schema.Literals(["initial", "reconnect", "resume"]),
  duration_ms: RendererDurationMs,
  event: Schema.Literal("forge_connection_finished"),
  failure_code: Schema.Literals([
    "timeout",
    "unavailable",
    "authentication_failed",
    "protocol_mismatch",
    "transport_error",
    "unknown",
  ]),
  outcome: Schema.Literal("failed"),
});

/** Records one use of a fixed product capability; custom integration names never cross the wire. */
export const FeatureUsedTelemetryIntent = Schema.Struct({
  event: Schema.Literal("feature_used"),
  feature: Schema.Literals([
    "subagent_graph",
    "checkpoint_rollback",
    "workspace_review",
    "routine",
    "capability",
    "preview",
    "thread_search",
  ]),
});

/** Closed renderer-originated event catalog accepted by Forge. */
export const TelemetryIntent = Schema.Union([
  EditorSessionStartedTelemetryIntent,
  ForgeConnectionSucceededTelemetryIntent,
  ForgeConnectionFailedTelemetryIntent,
  FeatureUsedTelemetryIntent,
]);
export type TelemetryIntent = typeof TelemetryIntent.Type;

/** Strictly decodes an untrusted renderer telemetry intent. */
export const DecodeTelemetryIntent = Schema.decodeUnknownEffect(TelemetryIntent, {
  onExcessProperty: "error",
});
