import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import { Database } from "../persistence/database";
import { PreviewInspectionSessions, PreviewTargets } from "../persistence/tables";
import {
	type PreviewInspectionProjection,
	PreviewRepositoryError,
	type PreviewTargetProjection,
} from "./contracts";
import { DecodeInspection, DecodeTarget } from "./storage-codec";

export class PreviewReader extends Context.Service<
	PreviewReader,
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
	}
>()("Artisan/PreviewReader") {}

const StorageError = (message: string) => (error: unknown) =>
	error instanceof PreviewRepositoryError
		? error
		: new PreviewRepositoryError({ code: "storage", message });

export const PreviewReaderLive = Layer.effect(
	PreviewReader,
	Effect.gen(function* () {
		const database = yield* Database;
		return {
			GetTarget: (target_id) =>
				database.client
					.select()
					.from(PreviewTargets)
					.where(eq(PreviewTargets.target_id, target_id))
					.limit(1)
					.pipe(
						Effect.flatMap(([row]) =>
							row === undefined
								? Effect.fail(
										new PreviewRepositoryError({
											code: "not_found",
											message: "Preview target not found",
										}),
									)
								: DecodeTarget(row),
						),
						Effect.mapError(StorageError("Could not read preview target")),
					),
			ListTargets: (workspace_id) =>
				(workspace_id === undefined
					? database.client
							.select()
							.from(PreviewTargets)
							.orderBy(asc(PreviewTargets.target_id))
					: database.client
							.select()
							.from(PreviewTargets)
							.where(eq(PreviewTargets.workspace_id, workspace_id))
							.orderBy(asc(PreviewTargets.target_id))
				).pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeTarget)),
					Effect.mapError(StorageError("Could not list preview targets")),
				),
			ListOpenInspections: () =>
				database.client
					.select()
					.from(PreviewInspectionSessions)
					.where(eq(PreviewInspectionSessions.state, "open"))
					.orderBy(asc(PreviewInspectionSessions.opened_at))
					.pipe(
						Effect.flatMap((rows) => Effect.forEach(rows, DecodeInspection)),
						Effect.mapError(StorageError("Could not list preview inspections")),
					),
		};
	}),
);
