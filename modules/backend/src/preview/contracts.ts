import { Context, Data, Effect, Option, Schema } from "effect";

const Identifier = Schema.String.pipe(Schema.check(Schema.isMinLength(1), Schema.isMaxLength(256)));
export const PreviewRoutes = Schema.Array(
	Schema.String.pipe(Schema.check(Schema.isMinLength(1), Schema.isMaxLength(2_048))),
).check(Schema.isMaxLength(128));
export const PreviewSource = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("process"), process_id: Identifier }),
	Schema.Struct({ kind: Schema.Literal("terminal"), terminal_id: Identifier }),
]);

export const PreviewTargetProjection = Schema.Struct({
	target_id: Identifier,
	thread_id: Identifier,
	project_id: Identifier,
	workspace_id: Identifier,
	url: Schema.String,
	port: Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 65_535 })),
	routes_json: Schema.String,
	source: Schema.optional(PreviewSource),
	state: Schema.Literals(["registered", "healthy", "unhealthy", "stopped", "removed"]),
	launch_state: Schema.Literals(["idle", "launching", "launched", "unavailable", "error"]),
	last_error: Schema.NullOr(Schema.String),
	health_json: Schema.NullOr(Schema.String),
	journal_sequence: Schema.Int.check(Schema.isGreaterThan(0)),
	created_at: Schema.String,
	updated_at: Schema.String,
	removed_at: Schema.NullOr(Schema.String),
});
export type PreviewTargetProjection = Schema.Schema.Type<typeof PreviewTargetProjection>;

export const PreviewInspectionProjection = Schema.Struct({
	session_id: Identifier,
	target_id: Identifier,
	thread_id: Identifier,
	connector_id: Identifier,
	state: Schema.Literals(["open", "closed", "abandoned"]),
	reconnect_state: Schema.Literals(["connected", "reconnecting", "unavailable", "error"]),
	last_error: Schema.NullOr(Schema.String),
	journal_sequence: Schema.Int.check(Schema.isGreaterThan(0)),
	opened_at: Schema.String,
	closed_at: Schema.NullOr(Schema.String),
	updated_at: Schema.String,
});
export type PreviewInspectionProjection = Schema.Schema.Type<typeof PreviewInspectionProjection>;

export const PreviewDispatchLease = Schema.Struct({
	acquired_at: Schema.String,
	expires_at: Schema.String,
	kind: Schema.Literals(["launch", "probe", "inspection_open", "inspection_health"]),
	lease_id: Schema.String,
	owner_instance_id: Schema.String,
	session_id: Schema.NullOr(Schema.String),
	target_id: Schema.NullOr(Schema.String),
	thread_id: Schema.String,
});
export type PreviewDispatchLease = Schema.Schema.Type<typeof PreviewDispatchLease>;

export const preview_dispatch_lease_duration_ms = 60_000;

export class PreviewRepositoryError extends Data.TaggedError("PreviewRepositoryError")<{
	readonly code: "invalid" | "not_found" | "storage";
	readonly message: string;
}> {}

export interface PreviewRegisterCommand {
	readonly message_id: string;
	readonly port: number;
	readonly project_id: string;
	readonly routes?: ReadonlyArray<string>;
	readonly source?: Schema.Schema.Type<typeof PreviewSource>;
	readonly target_id: string;
	readonly thread_id: string;
	readonly url: string;
	readonly workspace_id: string;
}
export interface PreviewTargetUpdateCommand {
	readonly action: "launch" | "probe" | "remove" | "state";
	readonly health_json?: string;
	readonly last_error?: string;
	readonly launch_state?: "idle" | "launching" | "launched" | "unavailable" | "error";
	readonly message_id: string;
	readonly state?: "healthy" | "registered" | "stopped" | "unhealthy" | "removed";
	readonly target_id: string;
	readonly thread_id: string;
}
export interface PreviewInspectionCommand {
	readonly action: "inspection_close" | "inspection_open" | "inspection_reconnect";
	readonly connector_id?: string;
	readonly last_error?: string;
	readonly message_id: string;
	readonly reconnect_state?: "connected" | "error" | "reconnecting" | "unavailable";
	readonly session_id: string;
	readonly target_id?: string;
	readonly thread_id: string;
}
export interface PreviewDispatchLeaseInput {
	readonly kind: PreviewDispatchLease["kind"];
	readonly session_id?: string;
	readonly target_id?: string;
	readonly thread_id: string;
}

export class PreviewRepository extends Context.Service<
	PreviewRepository,
	{
		readonly GetTarget: (
			target_id: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly ListTargets: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly ListOpenInspections: () => Effect.Effect<
			ReadonlyArray<PreviewInspectionProjection>,
			PreviewRepositoryError
		>;
		readonly Register: (
			input: PreviewRegisterCommand,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly ReplayTargetUpdate: (
			input: PreviewTargetUpdateCommand,
		) => Effect.Effect<Option.Option<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly UpdateTarget: (
			input: PreviewTargetUpdateCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly UpdateInspection: (
			input: PreviewInspectionCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewInspectionProjection, PreviewRepositoryError>;
		readonly RecoverInspections: () => Effect.Effect<
			ReadonlyArray<PreviewInspectionProjection>,
			PreviewRepositoryError
		>;
		readonly AcquireDispatchLease: (
			input: PreviewDispatchLeaseInput,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly ReleaseDispatchLease: (lease: PreviewDispatchLease) => Effect.Effect<void>;
		readonly RenewDispatchLease: (
			lease: PreviewDispatchLease,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly RecoverDispatchLeases: () => Effect.Effect<
			ReadonlyArray<PreviewDispatchLease>,
			PreviewRepositoryError
		>;
	}
>()("Artisan/PreviewRepository") {}
