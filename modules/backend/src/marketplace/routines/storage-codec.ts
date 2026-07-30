import { Effect, Schema } from "effect";
import { RoutineDetail, RoutineSummary, type ProviderSyncState } from "@artisan/protocol";
import { MarketplaceRoutines } from "../../persistence/tables";
import { RoutineRepositoryError } from "./contracts";

export const DecodeSummary = (row: typeof MarketplaceRoutines.$inferSelect) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.scope_json).pipe(
		Effect.flatMap((scope) =>
			Schema.decodeUnknownEffect(RoutineSummary, {
				onExcessProperty: "error",
			})({
				description: row.description,
				display_name: row.display_name,
				enabled: row.enabled,
				id: row.id,
				scope,
				status: row.status,
				version: row.version,
			}),
		),
		Effect.mapError(
			() =>
				new RoutineRepositoryError({
					code: "invariant",
					message: `Routine ${row.id} contains invalid persisted metadata`,
				}),
		),
	);

export const DecodeDetail = (
	row: typeof MarketplaceRoutines.$inferSelect,
	sync: ReadonlyArray<ProviderSyncState>,
) =>
	Effect.all({
		compatibility: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
			row.compatibility_json,
		).pipe(Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.compatibility))),
		commands: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.commands_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.exported_commands)),
		),
		files: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.files_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.files)),
		),
		permissions: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
			row.permissions_json,
		).pipe(Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.permissions))),
		scope: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.scope_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.scope)),
		),
		source: Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(row.source_json).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(RoutineDetail.fields.source)),
		),
	}).pipe(
		Effect.flatMap(({ compatibility, commands, files, permissions, scope, source }) =>
			Schema.decodeUnknownEffect(RoutineDetail, { onExcessProperty: "error" })({
				...(row.author === null ? {} : { author: row.author }),
				compatibility,
				description: row.description,
				display_name: row.display_name,
				enabled: row.enabled,
				exported_commands: commands,
				files,
				id: row.id,
				instructions: row.instructions,
				permissions,
				...(row.removed_at === null ? {} : { removed_at: row.removed_at }),
				scope,
				status: row.status,
				source,
				sync,
				trust: row.trust,
				version: row.version,
			}),
		),
		Effect.mapError(
			() =>
				new RoutineRepositoryError({
					code: "invariant",
					message: `Routine ${row.id} contains invalid persisted metadata`,
				}),
		),
	);

/** SQLite persistence for pre-write approval state. The installer is deliberately not a dependency. */
