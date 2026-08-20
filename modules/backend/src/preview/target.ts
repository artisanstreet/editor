import { Context, Data, Effect, Option, Scope, Stream } from "effect";

/** Identifies one preview target lifecycle state. */
export type PreviewTargetState = "healthy" | "registered" | "stopped" | "unhealthy";

/** Identifies the local process-like source that owns a preview target. */
export type PreviewTargetSource =
	| { readonly kind: "process"; readonly process_id: string }
	| { readonly kind: "terminal"; readonly terminal_id: string };

/** Records one bounded health observation for a preview target. */
export interface PreviewTargetHealth {
	readonly checked_at_ms: number;
	readonly latency_ms: number;
	readonly message: Option.Option<string>;
	readonly status: "healthy" | "unhealthy";
	readonly status_code: Option.Option<number>;
}

/** Defines one explicitly registered local preview target. */
export interface PreviewTargetRecord {
	readonly created_at_ms: number;
	readonly health: Option.Option<PreviewTargetHealth>;
	readonly id: string;
	readonly project_id: string;
	readonly source: Option.Option<PreviewTargetSource>;
	readonly state: PreviewTargetState;
	readonly updated_at_ms: number;
	readonly url: string;
	readonly workspace_id: string;
}

/** Supplies the immutable fields needed to register a preview target. */
export interface PreviewTargetRegistration {
	readonly id: string;
	readonly project_id: string;
	readonly source?: PreviewTargetSource;
	readonly url: string;
	readonly workspace_id: string;
}

/** Reports one registry change through the status stream. */
export interface PreviewTargetEvent {
	readonly kind: "health" | "registered" | "removed" | "state";
	readonly target: PreviewTargetRecord;
}

/** Identifies a preview registry or health-probe failure. */
export type PreviewTargetErrorCode = "duplicate" | "health_probe" | "invalid_target" | "not_found";

/** Reports a provider-neutral preview target failure. */
export class PreviewTargetError extends Data.TaggedError("PreviewTargetError")<{
	readonly cause: unknown;
	readonly code: PreviewTargetErrorCode;
	readonly target_id: string;
}> {}

/** Reports a failure from a replaceable local health probe. */
export class PreviewHealthProbeError extends Data.TaggedError("PreviewHealthProbeError")<{
	readonly cause: unknown;
	readonly target_id: string;
}> {}

/** Contains adapter output before the registry adds timestamps and state. */
export interface PreviewHealthProbeResult {
	readonly latency_ms: number;
	readonly message: Option.Option<string>;
	readonly status: "healthy" | "unhealthy";
	readonly status_code: Option.Option<number>;
}

/** Runs one scope-owned health observation without launching a server. */
export class PreviewHealthProbe extends Context.Service<
	PreviewHealthProbe,
	{
		readonly Probe: (
			target: PreviewTargetRecord,
		) => Effect.Effect<PreviewHealthProbeResult, PreviewHealthProbeError, Scope.Scope>;
	}
>()("Artisan/PreviewHealthProbe") {}

/** Supplies epoch milliseconds for preview target timestamps. */
export class PreviewTargetClock extends Context.Service<
	PreviewTargetClock,
	{
		readonly Now: Effect.Effect<number>;
	}
>()("Artisan/PreviewTargetClock") {}

/** Stores explicit local targets and exposes a stream that drops oldest unread events. */
export class PreviewTarget extends Context.Service<
	PreviewTarget,
	{
		readonly SlidingEvents: Stream.Stream<PreviewTargetEvent>;
		readonly Get: (id: string) => Effect.Effect<Option.Option<PreviewTargetRecord>>;
		readonly List: (workspace_id?: string) => Effect.Effect<ReadonlyArray<PreviewTargetRecord>>;
		readonly Probe: (
			id: string,
		) => Effect.Effect<PreviewTargetRecord, PreviewTargetError, Scope.Scope>;
		readonly Register: (
			input: PreviewTargetRegistration,
		) => Effect.Effect<PreviewTargetRecord, PreviewTargetError>;
		readonly Remove: (id: string) => Effect.Effect<void, PreviewTargetError>;
		readonly SetState: (
			id: string,
			state: PreviewTargetState,
		) => Effect.Effect<PreviewTargetRecord, PreviewTargetError>;
	}
>()("Artisan/PreviewTarget") {}
