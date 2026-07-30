import { and, asc, desc, eq } from "drizzle-orm";
import { Effect } from "effect";

import { JournalEvents, WorkspaceChanges, WorkspaceConflicts } from "../../persistence/tables";
import { DecodeChange, DecodeConflict } from "./storage-codec";

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

import { WorkspaceChangeContext } from "./context";

export const MakeQuery = Effect.gen(function* () {
	const context = yield* WorkspaceChangeContext;
	const { EnsureLiveThread, database, normalize_error } = context;

	const List = (thread_id: string, workspace_id?: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					yield* EnsureLiveThread(transaction, thread_id);

					const rows = yield* (
						workspace_id === undefined
							? transaction
									.select()
									.from(WorkspaceChanges)
									.where(eq(WorkspaceChanges.thread_id, thread_id))
							: transaction
									.select()
									.from(WorkspaceChanges)
									.where(
										and(
											eq(WorkspaceChanges.thread_id, thread_id),
											eq(WorkspaceChanges.workspace_id, workspace_id),
										),
									)
					).orderBy(desc(WorkspaceChanges.updated_at), asc(WorkspaceChanges.change_id));
					const [latest] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return {
						changes: yield* Effect.forEach(rows, DecodeChange),
						journal_sequence: latest?.journal_sequence ?? 0,
					};
				}),
			)
			.pipe(Effect.mapError(normalize_error));

	const ListConflicts = (thread_id: string) =>
		database.client
			.select()
			.from(WorkspaceConflicts)
			.where(eq(WorkspaceConflicts.attempting_thread_id, thread_id))
			.orderBy(asc(WorkspaceConflicts.detected_at), asc(WorkspaceConflicts.conflict_id))
			.pipe(
				Effect.flatMap((rows) => Effect.forEach(rows, DecodeConflict)),
				Effect.mapError(normalize_error),
			);

	const ListConflictSnapshot = (thread_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(WorkspaceConflicts)
						.where(eq(WorkspaceConflicts.attempting_thread_id, thread_id))
						.orderBy(
							asc(WorkspaceConflicts.detected_at),
							asc(WorkspaceConflicts.conflict_id),
						);
					const [latest] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return {
						conflicts: yield* Effect.forEach(rows, DecodeConflict),
						journal_sequence: latest?.journal_sequence ?? 0,
					};
				}),
			)
			.pipe(Effect.mapError(normalize_error));

	return { List, ListConflictSnapshot, ListConflicts };
});
