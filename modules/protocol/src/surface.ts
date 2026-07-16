import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

const text_encoder = new TextEncoder();

/** Defines the maximum visible character count for canonical surface labels. */
export const surface_label_maximum_characters = 256;

/** Defines the maximum UTF-8 byte count for canonical surface summaries. */
export const surface_summary_maximum_bytes = 2_048;

/** Defines the maximum UTF-8 byte count for one source-safe raw-origin field. */
export const surface_raw_origin_identifier_maximum_bytes = 256;

/** Defines the maximum UTF-8 byte count for one surface or ownership identifier. */
export const surface_identifier_maximum_bytes = 2_048;

const visible_text = (maximum_characters: number, maximum_bytes: number, description: string) =>
	Schema.String.check(
		Schema.makeFilter<string>((value) =>
			value.trim().length === 0 ||
			[...value].length > maximum_characters ||
			text_encoder.encode(value).byteLength > maximum_bytes ||
			/[\p{Cc}\p{Cf}]/u.test(value)
				? `Expected a non-empty ${description} within bounded visible text limits`
				: undefined,
		),
	);

const bounded_identifier = (maximum_bytes: number, description: string) =>
	Identifier.check(
		Schema.makeFilter<string>((value) =>
			text_encoder.encode(value).byteLength > maximum_bytes || /[\p{Cc}\p{Cf}]/u.test(value)
				? `Expected a ${description} within ${maximum_bytes} UTF-8 bytes without hidden control characters`
				: undefined,
		),
	);

/** Classifies the canonical Artisan-owned surface groups. */
export const SurfaceGroup = Schema.Literals([
	"Work",
	"Agents",
	"Time",
	"Guidance",
	"Routines",
	"Capabilities",
	"Engines",
	"Workspace",
	"Processes",
	"Changes",
	"Permissions",
	"Knowledge",
	"Identity",
	"Settings",
]);

export type SurfaceGroup = typeof SurfaceGroup.Type;

/** Enumerates product-owned nouns used by SurfaceItem projections. */
export const SurfaceKind = Schema.Literals([
	"thread",
	"message",
	"run",
	"agent",
	"timer",
	"guidance",
	"routine",
	"capability",
	"engine",
	"workspace",
	"process",
	"change",
	"approval",
	"knowledge",
	"identity",
	"setting",
	"preview",
	"question",
]);

export type SurfaceKind = typeof SurfaceKind.Type;

/** Identifies the source family without exposing provider-native payloads. */
export const SurfaceSource = Schema.Literals(["artisan", "engine", "marketplace"]);

export type SurfaceSource = typeof SurfaceSource.Type;

/** Validates one bounded canonical label. */
export const SurfaceLabel = visible_text(surface_label_maximum_characters, 512, "surface label");

/** Validates one bounded source-safe canonical summary. */
export const SurfaceSummary = visible_text(1_024, surface_summary_maximum_bytes, "surface summary");

/** Validates a compact canonical lifecycle state. */
export const SurfaceState = visible_text(64, 128, "surface state");

/** Validates a bounded identifier carried by a source-safe surface projection. */
export const SurfaceIdentifier = bounded_identifier(
	surface_identifier_maximum_bytes,
	"surface identifier",
);

/** Carries bounded provider-neutral token totals for one Work run. */
export const SurfaceUsage = Schema.Struct({
	input_tokens: Schema.Int.pipe(
		Schema.check(Schema.isGreaterThanOrEqualTo(0)),
		Schema.check(Schema.isLessThanOrEqualTo(Number.MAX_SAFE_INTEGER)),
	),
	output_tokens: Schema.Int.pipe(
		Schema.check(Schema.isGreaterThanOrEqualTo(0)),
		Schema.check(Schema.isLessThanOrEqualTo(Number.MAX_SAFE_INTEGER)),
	),
});

export type SurfaceUsage = typeof SurfaceUsage.Type;

/** Retains only bounded attribution needed to trace a surface item to its raw source. */
export const SurfaceRawOrigin = Schema.Struct({
	provider: bounded_identifier(
		surface_raw_origin_identifier_maximum_bytes,
		"raw-origin provider",
	),
	reference: bounded_identifier(
		surface_raw_origin_identifier_maximum_bytes,
		"raw-origin reference",
	),
});

export type SurfaceRawOrigin = typeof SurfaceRawOrigin.Type;

/** Projects one source-safe, provider-neutral item for the Artisan interface. */
export const SurfaceItem = Schema.Struct({
	agent_id: Schema.optional(SurfaceIdentifier),
	group: SurfaceGroup,
	kind: SurfaceKind,
	label: SurfaceLabel,
	project_id: Schema.optional(SurfaceIdentifier),
	raw_origin: Schema.optional(SurfaceRawOrigin),
	run_id: Schema.optional(SurfaceIdentifier),
	source: SurfaceSource,
	state: SurfaceState,
	summary: SurfaceSummary,
	surface_id: SurfaceIdentifier,
	thread_id: Schema.optional(SurfaceIdentifier),
	timestamp: IsoDateTime,
	usage: Schema.optional(SurfaceUsage),
	workspace_id: Schema.optional(SurfaceIdentifier),
});

export type SurfaceItem = typeof SurfaceItem.Type;
