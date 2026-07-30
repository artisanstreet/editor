import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";
import { ProviderSyncState } from "@artisan/protocol";
import { Database } from "../../persistence/database";
import { MarketplaceRoutineMirrors, MarketplaceRoutines } from "../../persistence/tables";
import { DecodeDetail, DecodeSummary } from "./storage-codec";

import { RoutineRepositoryError, type RoutineRepositoryApi } from "./contracts";

export class RoutineCatalog extends Context.Service<
	RoutineCatalog,
	Pick<RoutineRepositoryApi, "ReadSummaries" | "ReadDetail">
>()("Artisan/Marketplace/Routines/RoutineCatalog") {}

export const RoutineCatalogLive = Layer.effect(
	RoutineCatalog,
	Effect.gen(function* () {
		const database = yield* Database;

		const ReadSummaries = database.client
			.select()
			.from(MarketplaceRoutines)
			.orderBy(asc(MarketplaceRoutines.display_name))
			.pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeSummary)),
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: "Routine registry could not be read",
							}),
				),
			);

		const ReadDetail = (routine_id: string) =>
			Effect.gen(function* () {
				const [row] = yield* database.client
					.select()
					.from(MarketplaceRoutines)
					.where(eq(MarketplaceRoutines.id, routine_id))
					.limit(1);
				if (!row) {
					return yield* new RoutineRepositoryError({
						code: "not_found",
						message: `Routine ${routine_id} was not found`,
					});
				}
				const mirrors = yield* database.client
					.select()
					.from(MarketplaceRoutineMirrors)
					.where(eq(MarketplaceRoutineMirrors.routine_id, routine_id));
				const sync = yield* Effect.forEach(mirrors, (mirror) =>
					Schema.decodeUnknownEffect(ProviderSyncState)({
						engine_id: mirror.engine_id,
						...(mirror.last_error_code === null
							? {}
							: { last_error_code: mirror.last_error_code }),
						...(mirror.observed_revision === null
							? {}
							: { observed_revision: mirror.observed_revision }),
						status: mirror.status,
						updated_at: mirror.updated_at,
					}),
				);
				return yield* DecodeDetail(row, sync);
			}).pipe(
				Effect.mapError((error) =>
					error instanceof RoutineRepositoryError
						? error
						: new RoutineRepositoryError({
								code: "invariant",
								message: `Routine ${routine_id} contains invalid persisted metadata`,
							}),
				),
			);

		return { ReadSummaries, ReadDetail };
	}),
);
