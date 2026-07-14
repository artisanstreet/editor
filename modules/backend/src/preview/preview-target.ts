import { Context, Data, Effect, Layer, Option, Scope, Stream } from "effect";

import {
	PreviewTargetHealth as PreviewTargetHealthSchema,
	type CommandEnvelope,
	type EventEnvelope,
	type PreviewTargetRecord,
	type PreviewTargetRemovedRecord,
} from "@artisan/protocol";

export type {
	PreviewTargetHealth,
	PreviewTargetRecord,
	PreviewTargetSource,
} from "@artisan/protocol";

/** Identifies one stored preview lifecycle state. */
export type PreviewTargetState = PreviewTargetRecord["state"];

/** Preserves the public error-code alias used by the package entrypoint. */
export type PreviewTargetErrorCode =
	| "conflict"
	| "health_probe"
	| "invalid_target"
	| "invariant"
	| "not_found"
	| "unavailable";

/** Preserves the event alias while events now use canonical journal envelopes. */
export type PreviewTargetEvent = EventEnvelope;

/** Preserves the registration alias while commands carry the durable identity. */
export type PreviewTargetRegistration = CommandEnvelope;

/** Represents a target snapshot carried by an accepted lifecycle event. */
export type PreviewTargetSnapshot = PreviewTargetRecord | PreviewTargetRemovedRecord;

/** Carries a protocol command whose message identity anchors durable replay. */
export type PreviewTargetCommandEnvelope = CommandEnvelope;

/** Captures a health observation before it is committed to a target. */
export interface PreviewHealthProbeResult {
	readonly latency_ms: number;
	readonly message: Option.Option<string>;
	readonly status: "healthy" | "unhealthy";
	readonly status_code: Option.Option<number>;
}

/** Returns the canonical committed event and whether this invocation created it. */
export interface PreviewTargetAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Reports a source-safe preview registry or health-probe failure. */
export class PreviewTargetError extends Data.TaggedError("PreviewTargetError")<{
	readonly code: PreviewTargetErrorCode;
	readonly target_id: string;
}> {}

/** Reports bounded local-health adapter failure without exposing implementation errors. */
export class PreviewHealthProbeError extends Data.TaggedError("PreviewHealthProbeError")<{
	readonly reason: "unavailable" | "failed";
	readonly target_id: string;
}> {}

/** Runs one scope-owned health observation without launching a server. */
export class PreviewHealthProbe extends Context.Service<
	PreviewHealthProbe,
	{
		readonly Probe: (
			target: PreviewTargetRecord,
		) => Effect.Effect<PreviewHealthProbeResult, PreviewHealthProbeError, Scope.Scope>;
	}
>()("Artisan/PreviewHealthProbe") {}

/** Provides the explicit default when no local preview health adapter is installed. */
export const UnavailablePreviewHealthProbeLive = Layer.succeed(PreviewHealthProbe, {
	Probe: (target) =>
		Effect.fail(
			new PreviewHealthProbeError({ reason: "unavailable", target_id: target.target_id }),
		),
});

/** Supplies epoch milliseconds for preview target timestamps. */
export class PreviewTargetClock extends Context.Service<
	PreviewTargetClock,
	{
		readonly Now: Effect.Effect<number>;
	}
>()("Artisan/PreviewTargetClock") {}

/** Owns durable preview targets and a bounded feed of newly committed events. */
export class PreviewTarget extends Context.Service<
	PreviewTarget,
	{
		readonly SlidingEvents: Stream.Stream<EventEnvelope>;
		readonly Get: (input: {
			readonly project_id: string;
			readonly target_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<Option.Option<PreviewTargetRecord>, PreviewTargetError>;
		readonly List: (input: {
			readonly project_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<ReadonlyArray<PreviewTargetRecord>, PreviewTargetError>;
		readonly Probe: (
			command: PreviewTargetCommandEnvelope,
		) => Effect.Effect<PreviewTargetAcceptance, PreviewTargetError, Scope.Scope>;
		readonly Register: (
			command: PreviewTargetCommandEnvelope,
		) => Effect.Effect<PreviewTargetAcceptance, PreviewTargetError>;
		readonly Remove: (
			command: PreviewTargetCommandEnvelope,
		) => Effect.Effect<PreviewTargetAcceptance, PreviewTargetError>;
	}
>()("Artisan/PreviewTarget") {}

/** Validates a probe result before it crosses the durable persistence boundary. */
export const PreviewHealthProbeResultSchema = PreviewTargetHealthSchema;
