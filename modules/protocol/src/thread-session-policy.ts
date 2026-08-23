import { Effect, Schema } from "effect";

import { Identifier } from "./common";

/** The durable, provider-neutral launch policy selected for one thread. */
export const ThreadSessionPolicy = Schema.Struct({
	/** Revision of the runtime catalog from which the structured selection came. */
	catalog_revision: Schema.optional(Schema.NonEmptyString),
	/** Any engine identifier decodes; the runtime catalog rejects unregistered engines. */
	engine_id: Identifier,
	/** Opaque model id within `provider_route_id`; required for route-aware engines. */
	model_id: Schema.optional(Schema.NonEmptyString),
	model: Schema.optional(Schema.NonEmptyString),
	/** Artisan toolchain/config/credential profile; required for route-aware engines. */
	profile_id: Schema.optional(Schema.NonEmptyString),
	/** Execution/billing route, distinct from the underlying model provider. */
	provider_route_id: Schema.optional(Schema.NonEmptyString),
	/**
	 * The native context-window suffix appended to the model id (for example
	 * Claude Code's `[1m]`). Absent means the harness resolves the bare model
	 * id itself, which lands on the catalog capability's default option — for
	 * Claude 5 that is the extended window, not the 200K base one.
	 */
	context_window: Schema.optional(Schema.NonEmptyString),
	reasoning_effort: Schema.Literals(["low", "medium", "high", "xhigh", "max", "ultra"]),
	/**
	 * The harness-neutral permission option id from the catalog manifest, and
	 * the authoritative permission choice. `permission_mode` and `sandbox_mode`
	 * are the two coarse compatibility axes derived from it for sandbox and tool
	 * gating. Retired five-step ids normalize onto the current three-mode scale.
	 */
	permission: Identifier.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("autonomous")),
	),
	permission_mode: Schema.Literals(["never", "on_request"]),
	sandbox_mode: Schema.Literals(["read_only", "workspace_write"]),
	service_tier: Schema.NonEmptyString.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("standard")),
	),
	web_search_enabled: Schema.Boolean,
	strict_clarification: Schema.Boolean,
	/** Optional harness-native model overlay; never reconstructed from reasoning effort. */
	variant_id: Schema.optional(Schema.NonEmptyString),
});

export type ThreadSessionPolicy = typeof ThreadSessionPolicy.Type;

/** Collapses retired five-step choices onto the current three-mode contract. */
export const NormalizePermissionId = (permission: string) =>
	permission === "supervised" || permission === "trusted" ? "autonomous" : permission;

/** Reads the neutral option represented by policies written before `permission` existed. */
export const SessionPolicyPermission = (policy: ThreadSessionPolicy) =>
	NormalizePermissionId(
		policy.permission ?? (policy.sandbox_mode === "read_only" ? "restricted" : "autonomous"),
	);
