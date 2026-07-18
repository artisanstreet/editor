import { Schema } from "effect";

import { Identifier, IsoDateTime, RawOrigin } from "./common";

/** Limits public surface text to compact, visible summaries rather than provider payloads. */
export const SurfaceSafeText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 && value.length <= 4_096 && !/[\p{Cc}]/u.test(value)
			? undefined
			: "Expected between 1 and 4096 visible characters",
	),
);

export type SurfaceSafeText = typeof SurfaceSafeText.Type;

/** Identifies the Artisan-owned product group for one canonical surface item. */
export const SurfaceCategory = Schema.Literals([
	"work",
	"time",
	"guidance",
	"routine",
	"capability",
	"process",
	"change",
	"permission",
	"native_action",
]);

export type SurfaceCategory = typeof SurfaceCategory.Type;

/** Names the canonical Work surface kinds. */
export const WorkSurfaceKind = Schema.Literals([
	"thread",
	"message",
	"turn",
	"run",
	"plan",
	"agent",
	"assignment",
	"group",
	"join",
	"artifact",
]);

/** Names the canonical Time surface kinds. */
export const TimeSurfaceKind = Schema.Literals([
	"timer",
	"reminder",
	"monitor",
	"scheduled_run",
	"follow_up",
	"recurring_check",
]);

/** Names the canonical Guidance surface kinds. */
export const GuidanceSurfaceKind = Schema.Literals([
	"global_guidance",
	"project_guidance",
	"thread_guidance",
	"instruction_file",
	"prompt_template",
	"provider_guidance",
]);

/** Names the canonical Routine surface kinds. */
export const RoutineSurfaceKind = Schema.Literals([
	"skill",
	"slash_command",
	"reusable_prompt",
	"workflow",
	"recipe",
	"routine_invocation",
]);

/** Names the canonical Capability surface kinds. */
export const CapabilitySurfaceKind = Schema.Literals([
	"tool",
	"mcp_server",
	"connector",
	"browser",
	"computer_use",
	"web_search",
	"native_tool",
]);

/** Names the canonical Process surface kinds. */
export const ProcessSurfaceKind = Schema.Literals([
	"terminal",
	"shell_command",
	"dev_server",
	"background_task",
	"process_log",
]);

/** Names the canonical Change surface kinds. */
export const ChangeSurfaceKind = Schema.Literals([
	"workspace_change",
	"file_observation",
	"diff",
	"patch",
	"git_status",
	"commit",
	"branch",
	"stash",
	"pull_request",
	"workspace_conflict",
	"review",
]);

/** Names the canonical Permission surface kinds. */
export const PermissionSurfaceKind = Schema.Literals([
	"approval",
	"sandbox",
	"trust_prompt",
	"destructive_action",
	"secret",
	"credential",
	"access_scope",
]);

/** Names the canonical fallback kinds for engine behavior Artisan cannot classify safely. */
export const NativeActionSurfaceKind = Schema.Literals([
	"engine_native_action",
	"opaque_engine_work",
	"provider_diagnostic",
	"compaction",
	"model_reroute",
]);

/** Keeps public surface attribution independent from any provider-native identity. */
export const SurfaceAttribution = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	assignment_id: Schema.optional(Identifier),
	group_id: Schema.optional(Identifier),
	run_id: Schema.optional(Identifier),
	thread_id: Schema.optional(Identifier),
	workspace_id: Schema.optional(Identifier),
});

export type SurfaceAttribution = typeof SurfaceAttribution.Type;

/** Links a surface item to raw provider evidence without copying the raw frame into public state. */
export const SurfaceRawObservationReference = Schema.Struct({
	engine_id: Identifier,
	observation_id: Identifier,
});

export type SurfaceRawObservationReference = typeof SurfaceRawObservationReference.Type;

/** Represents a bounded public summary that is safe to project without provider payloads. */
export const SurfaceSafeSummary = Schema.Struct({
	detail: Schema.optional(SurfaceSafeText),
	label: SurfaceSafeText,
	status: Schema.optional(SurfaceSafeText),
});

export type SurfaceSafeSummary = typeof SurfaceSafeSummary.Type;

const SurfaceItemBase = {
	attribution: SurfaceAttribution,
	causation_id: Schema.optional(Identifier),
	correlation_id: Schema.optional(Identifier),
	occurred_at: IsoDateTime,
	raw_observation: Schema.optional(SurfaceRawObservationReference),
	raw_origin: Schema.optional(RawOrigin),
	surface_id: Identifier,
};

/** Represents a strict canonical Work surface item. */
export const WorkSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("work"),
	kind: WorkSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Time surface item. */
export const TimeSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("time"),
	kind: TimeSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Guidance surface item. */
export const GuidanceSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("guidance"),
	kind: GuidanceSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Routine surface item. */
export const RoutineSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("routine"),
	kind: RoutineSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Capability surface item. */
export const CapabilitySurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("capability"),
	kind: CapabilitySurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Process surface item. */
export const ProcessSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("process"),
	kind: ProcessSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Change surface item. */
export const ChangeSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("change"),
	kind: ChangeSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents a strict canonical Permission surface item. */
export const PermissionSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("permission"),
	kind: PermissionSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Represents safely retained engine work that has no more specific canonical category. */
export const NativeActionSurfaceItem = Schema.Struct({
	...SurfaceItemBase,
	category: Schema.Literal("native_action"),
	kind: NativeActionSurfaceKind,
	summary: SurfaceSafeSummary,
});

/** Unions every strict provider-neutral canonical surface item. */
export const SurfaceItem = Schema.Union([
	WorkSurfaceItem,
	TimeSurfaceItem,
	GuidanceSurfaceItem,
	RoutineSurfaceItem,
	CapabilitySurfaceItem,
	ProcessSurfaceItem,
	ChangeSurfaceItem,
	PermissionSurfaceItem,
	NativeActionSurfaceItem,
]);

export type SurfaceItem = typeof SurfaceItem.Type;

/** Query-ready ordered projection snapshot; transport envelopes remain elsewhere. */
export const SurfaceSnapshot = Schema.Struct({
	items: Schema.Array(SurfaceItem),
	journal_sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});
export type SurfaceSnapshot = typeof SurfaceSnapshot.Type;

/** Selects renderer-safe surface items for one thread, optionally narrowed to ownership. */
export const SurfaceListQuery = Schema.Struct({
	thread_id: Identifier,
	run_id: Schema.optional(Identifier),
	group_id: Schema.optional(Identifier),
});
export type SurfaceListQuery = typeof SurfaceListQuery.Type;

/** Optional token totals faithfully reported by a provider. */
export const SurfaceUsage = Schema.Struct({
	assignment_id: Schema.optional(Identifier),
	group_id: Schema.optional(Identifier),
	input_tokens: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
	output_tokens: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
	run_id: Identifier,
	updated_at: IsoDateTime,
});
export type SurfaceUsage = typeof SurfaceUsage.Type;

/** Aggregate token totals for one run, assignment, or group; omitted metrics remain unknown. */
export const SurfaceUsageAggregate = Schema.Struct({
	scope: Schema.Literals(["run", "assignment", "group"]),
	scope_id: Identifier,
	input_tokens: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
	output_tokens: Schema.optional(Schema.Int.check(Schema.isGreaterThanOrEqualTo(0))),
});
export type SurfaceUsageAggregate = typeof SurfaceUsageAggregate.Type;

export const SurfaceUsageAggregateQuery = Schema.Struct({
	scope: Schema.Literals(["run", "assignment", "group"]),
	scope_id: Identifier,
});
export type SurfaceUsageAggregateQuery = typeof SurfaceUsageAggregateQuery.Type;

export const SurfaceUsageAggregateSnapshot = Schema.Struct({
	aggregate: SurfaceUsageAggregate,
	journal_sequence: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});
export type SurfaceUsageAggregateSnapshot = typeof SurfaceUsageAggregateSnapshot.Type;
