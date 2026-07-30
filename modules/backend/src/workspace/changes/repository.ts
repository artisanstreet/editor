import { Effect, Layer } from "effect";

import { WorkspaceChangeRepository } from "./model";
import { MakeClaim } from "./claim";
import { MakeCommit } from "./commit";
import { MakeWorkspaceChangeContext, WorkspaceChangeContext } from "./context";
import { MakeQuery } from "./query";
import { MakeReconciliation } from "./reconciliation";

export {
	WorkspaceChangeIdConflict,
	WorkspaceChangeRepository,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type ClaimReview,
	type ClaimRollback,
	type ReconcileWorkspaceChange,
	type WorkspaceChangeClaim,
	type WorkspaceChangeCommit,
	type WorkspaceChangeEvent,
	type WorkspaceChangeOperation,
	type WorkspaceChangeReconciliation,
	type WorkspaceChangeRepositoryError,
} from "./model";

/** Supplies the SQLite-backed workspace change repository. */
export const WorkspaceChangeRepositoryLive = Layer.effect(
	WorkspaceChangeRepository,
	Effect.gen(function* () {
		const context = yield* MakeWorkspaceChangeContext;
		const ProvideContext = <A, E>(effect: Effect.Effect<A, E, WorkspaceChangeContext>) =>
			effect.pipe(Effect.provideService(WorkspaceChangeContext, context));
		const { Claim } = yield* ProvideContext(MakeClaim);
		const { Commit, MarkEvidenceRecorded } = yield* ProvideContext(MakeCommit);
		const { List, ListConflictSnapshot, ListConflicts } = yield* ProvideContext(MakeQuery);
		const { MarkApplied, ReconcileChanged, RejectChanged } =
			yield* ProvideContext(MakeReconciliation);

		return {
			ClaimReplace: Claim,
			ClaimReview: Claim,
			ClaimRollback: Claim,
			CommitRecorded: (message_id, prepared_diff) =>
				Commit(message_id, "recorded", prepared_diff),
			CommitReviewed: (message_id) => Commit(message_id, "reviewed"),
			CommitRolledBack: (message_id) => Commit(message_id, "rolled_back"),
			List,
			ListConflictSnapshot,
			ListConflicts,
			MarkApplied,
			MarkEvidenceRecorded,
			ReconcileChanged,
			RejectChanged,
			ReadChange: context.ReadChange,
			ReadOperation: context.ReadOperation,
		};
	}),
);
