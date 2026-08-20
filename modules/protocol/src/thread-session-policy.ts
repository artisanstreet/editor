import { Effect, Schema } from "effect";

import { Identifier } from "./common";

/** The durable, provider-neutral launch policy selected for one thread. */
export const ThreadSessionPolicy = Schema.Struct({
	/** Any engine identifier decodes; the runtime catalog rejects unregistered engines. */
	engine_id: Identifier,
	model: Schema.optional(Schema.NonEmptyString),
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
	 * are the two coarse axes derived from it for sandbox and tool gating; they
	 * cannot express every harness option (Claude alone has five), so nothing
	 * may reconstruct the option id from them.
	 */
	permission: Identifier.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("supervised")),
	),
	permission_mode: Schema.Literals(["never", "on_request"]),
	sandbox_mode: Schema.Literals(["read_only", "workspace_write"]),
	service_tier: Schema.NonEmptyString.pipe(
		Schema.optional,
		Schema.withDecodingDefault(Effect.succeed("standard")),
	),
	web_search_enabled: Schema.Boolean,
	strict_clarification: Schema.Boolean,
});

export type ThreadSessionPolicy = typeof ThreadSessionPolicy.Type;

/** Reads the neutral option represented by policies written before `permission` existed. */
export const SessionPolicyPermission = (policy: ThreadSessionPolicy) =>
	policy.permission ??
	(policy.sandbox_mode === "read_only"
		? "restricted"
		: policy.permission_mode === "never"
			? "autonomous"
			: "supervised");
