import { Effect, Schema } from "effect";

import { PreviewInspectionSessionUpdatedEvent, PreviewTargetUpdatedEvent } from "@artisan/protocol";

import { PreviewInspectionSessions, PreviewTargets } from "../persistence/tables";
import {
	PreviewInspectionProjection,
	PreviewRepositoryError,
	PreviewRoutes,
	PreviewSource,
	PreviewTargetProjection,
} from "./contracts";

const StoredTarget = Schema.Struct({
	target_id: Schema.String,
	thread_id: Schema.String,
	project_id: Schema.String,
	workspace_id: Schema.String,
	url: Schema.String,
	port: Schema.Number,
	routes_json: Schema.String,
	source_kind: Schema.NullOr(Schema.Literals(["process", "terminal"])),
	source_id: Schema.NullOr(Schema.String),
	state: Schema.String,
	launch_state: Schema.String,
	last_error: Schema.NullOr(Schema.String),
	health_json: Schema.NullOr(Schema.String),
	journal_sequence: Schema.Number,
	created_at: Schema.String,
	updated_at: Schema.String,
	removed_at: Schema.NullOr(Schema.String),
});

const DecodeJson = (value: string) => Schema.decodeUnknownSync(Schema.UnknownFromJsonString)(value);

export const RequireStored = <Value>(
	value: Value | undefined,
	message: string,
): Effect.Effect<Value, PreviewRepositoryError> =>
	value === undefined
		? Effect.fail(new PreviewRepositoryError({ code: "storage", message }))
		: Effect.succeed(value);

export const DecodeTargetEvent = (value: string) =>
	Schema.decodeUnknownEffect(Schema.fromJsonString(PreviewTargetUpdatedEvent))(value).pipe(
		Effect.mapError(
			() =>
				new PreviewRepositoryError({
					code: "storage",
					message: "Preview command journal event is invalid",
				}),
		),
	);

export const DecodeTarget = (value: unknown) =>
	Effect.try({
		try: () => {
			const row = Schema.decodeUnknownSync(StoredTarget)(value);
			Schema.decodeUnknownSync(PreviewRoutes)(DecodeJson(row.routes_json));
			const source =
				row.source_kind === null
					? undefined
					: Schema.decodeUnknownSync(PreviewSource)(
							row.source_kind === "process"
								? { kind: "process", process_id: row.source_id }
								: { kind: "terminal", terminal_id: row.source_id },
						);
			return Schema.decodeUnknownSync(PreviewTargetProjection)({
				...row,
				...(source === undefined ? {} : { source }),
			});
		},
		catch: () =>
			new PreviewRepositoryError({
				code: "storage",
				message: "Preview target projection is invalid",
			}),
	});

export const DecodeInspection = (value: unknown) =>
	Schema.decodeUnknownEffect(PreviewInspectionProjection)(value).pipe(
		Effect.mapError(
			() =>
				new PreviewRepositoryError({
					code: "storage",
					message: "Preview inspection projection is invalid",
				}),
		),
	);

export const TargetEventPayload = (row: typeof PreviewTargets.$inferSelect) =>
	Schema.decodeUnknownSync(PreviewTargetUpdatedEvent)({
		type: "preview.target.updated",
		target: {
			id: row.target_id,
			thread_id: row.thread_id,
			workspace_id: row.workspace_id,
			project_id: row.project_id,
			url: row.url,
			port: row.port,
			routes: Schema.decodeUnknownSync(PreviewRoutes)(DecodeJson(row.routes_json)),
			...(row.source_kind === null
				? {}
				: {
						source:
							row.source_kind === "process"
								? { kind: "process", process_id: row.source_id }
								: { kind: "terminal", terminal_id: row.source_id },
					}),
			state: row.state,
			launch_state: row.launch_state,
			...(row.last_error === null ? {} : { last_error: row.last_error }),
			...(row.health_json === null ? {} : { health: DecodeJson(row.health_json) }),
			journal_sequence: row.journal_sequence,
			created_at: row.created_at,
			updated_at: row.updated_at,
		},
	});

export const InspectionEventPayload = (row: typeof PreviewInspectionSessions.$inferSelect) =>
	Schema.decodeUnknownSync(PreviewInspectionSessionUpdatedEvent)({
		type: "preview.inspection.updated",
		session: {
			session_id: row.session_id,
			target_id: row.target_id,
			connector_id: row.connector_id,
			state: row.state,
			reconnect_state: row.reconnect_state,
			...(row.last_error === null ? {} : { last_error: row.last_error }),
			opened_at: row.opened_at,
			...(row.closed_at === null ? {} : { closed_at: row.closed_at }),
			updated_at: row.updated_at,
		},
	});

export const EncodeTargetEvent = (row: typeof PreviewTargets.$inferSelect) =>
	JSON.stringify(TargetEventPayload(row));

export const EncodeInspectionEvent = (row: typeof PreviewInspectionSessions.$inferSelect) =>
	JSON.stringify(InspectionEventPayload(row));
