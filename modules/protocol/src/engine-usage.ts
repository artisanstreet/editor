import { Schema } from "effect";

import { IsoDateTime } from "./common";

/** Classifies one provider quota window by its billing cadence. */
export const EngineUsageWindowKind = Schema.Literals(["session", "weekly", "monthly", "unknown"]);

export type EngineUsageWindowKind = typeof EngineUsageWindowKind.Type;

/**
 * Projects one provider-reported quota window as a used percentage.
 *
 * `id` is the provider's stable bucket identifier (for example `five_hour` or
 * `codex_bengalfox`). `label` carries the provider's human bucket name when one
 * exists (for example a model-scoped weekly limit such as `Fable`), so clients
 * render provider vocabulary instead of inventing their own.
 */
export const EngineUsageWindow = Schema.Struct({
	id: Schema.String.check(Schema.isMinLength(1)),
	kind: EngineUsageWindowKind,
	label: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	percent_used: Schema.Number.check(Schema.isBetween({ maximum: 100, minimum: 0 })),
	resets_at: Schema.optional(IsoDateTime),
	window_minutes: Schema.optional(Schema.Int.check(Schema.isGreaterThan(0))),
});

export type EngineUsageWindow = typeof EngineUsageWindow.Type;

/** Reports whether the provider account behind an engine can be billed for runs. */
export const EngineUsageAuthentication = Schema.Literals([
	"authenticated",
	"unauthenticated",
	"unknown",
]);

export type EngineUsageAuthentication = typeof EngineUsageAuthentication.Type;

/**
 * Projects one engine's provider-account usage. `windows` is empty when the
 * account is not authenticated or the provider exposes no quota surface;
 * `failure` explains an authenticated account whose usage fetch failed.
 */
export const EngineUsageReport = Schema.Struct({
	authentication: EngineUsageAuthentication,
	display_name: Schema.String.check(Schema.isMinLength(1)),
	engine_id: Schema.String.check(Schema.isMinLength(1)),
	failure: Schema.optional(Schema.String.check(Schema.isMinLength(1))),
	windows: Schema.Array(EngineUsageWindow).check(Schema.isMaxLength(64)),
});

export type EngineUsageReport = typeof EngineUsageReport.Type;

/** Requests provider-account usage for every registered engine. */
export const EngineUsageQuery = Schema.Struct({});

export type EngineUsageQuery = typeof EngineUsageQuery.Type;

/** Carries the provider-account usage reports for all registered engines. */
export const EngineUsageSnapshot = Schema.Struct({
	engines: Schema.Array(EngineUsageReport).check(Schema.isMaxLength(16)),
	fetched_at: IsoDateTime,
});

export type EngineUsageSnapshot = typeof EngineUsageSnapshot.Type;
