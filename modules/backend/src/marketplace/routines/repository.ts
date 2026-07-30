import { Context, Effect, Layer } from "effect";
import { RoutineApprovals, RoutineApprovalsLive } from "./approvals";
import { RoutineCatalog, RoutineCatalogLive } from "./catalog";
import { type RoutineRepositoryApi } from "./contracts";
import { RoutineInstallation, RoutineInstallationLive } from "./installation";
import { RoutineMirrors, RoutineMirrorsLive } from "./mirrors";
import { RoutineRecoveryOperations, RoutineRecoveryOperationsLive } from "./recovery";

export * from "./contracts";

export class RoutineRepository extends Context.Service<RoutineRepository, RoutineRepositoryApi>()(
	"Artisan/Marketplace/RoutineRepository",
) {}

const RoutineRepositoryComponentsLive = Layer.mergeAll(
	RoutineApprovalsLive,
	RoutineCatalogLive,
	RoutineInstallationLive,
	RoutineMirrorsLive,
	RoutineRecoveryOperationsLive,
);

export const RoutineRepositoryLive = Layer.effect(
	RoutineRepository,
	Effect.gen(function* () {
		const approvals = yield* RoutineApprovals;
		const catalog = yield* RoutineCatalog;
		const installation = yield* RoutineInstallation;
		const mirrors = yield* RoutineMirrors;
		const recovery = yield* RoutineRecoveryOperations;

		return { ...approvals, ...catalog, ...installation, ...mirrors, ...recovery };
	}),
).pipe(Layer.provide(RoutineRepositoryComponentsLive));
