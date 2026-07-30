import { and, asc, desc, eq, gte, ne } from "drizzle-orm";
import { Effect, Option, Schema } from "effect";

import {
	ContentIdentity,
	WorkspaceConflictUpdatedEvent,
	type ContentIdentity as ContentIdentityValue,
	type WorkspaceConflict as WorkspaceConflictValue,
	type WorkspaceConflictUpdatedEvent as WorkspaceConflictUpdatedEventValue,
} from "@artisan/protocol";

import { RetrySqliteWrite } from "../../persistence/sqlite-write-retry";
import {
	WorkspaceChangeOperations,
	WorkspaceChanges,
	WorkspaceConflicts,
	WorkspaceMutationAuthorities,
} from "../../persistence/tables";
import { JournalInvariantError } from "../../persistence/journal-store";
import {
	WorkspaceChangeTransitionError,
	type ReconcileWorkspaceChange,
	type WorkspaceChangeReconciliation,
} from "./model";
import { DecodeConflict, DecodeOperation } from "./storage-codec";

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

export const MakeReconciliation = Effect.gen(function* () {
	const context = yield* WorkspaceChangeContext;
	const {
		AppendJournalEventInTransaction,
		DecodeJson,
		EnsureLiveThread,
		HasAvailablePayload,
		ReadDuplicate,
		ValidateRejectedState,
		ValidateTransition,
		database,
		identities_match,
		metadata,
		normalize_error,
		notifier,
	} = context;

	const MarkApplied = (
		input:
			| {
					readonly _tag: "replace";
					readonly message_id: string;
					readonly result_identity: ContentIdentityValue;
			  }
			| { readonly _tag: "rollback"; readonly message_id: string },
	) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
						.limit(1);

					if (!row)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${input.message_id} is missing`,
						});

					const operation = yield* DecodeOperation(row);

					yield* EnsureLiveThread(transaction, operation.thread_id);

					if (operation.action !== input._tag)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${input.message_id} cannot be applied`,
						});

					if (
						input._tag === "replace" &&
						(operation.action !== "replace" ||
							!identities_match(operation.result_identity, input.result_identity))
					) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${input.message_id} did not produce its intended result`,
						});
					}

					if (operation.action === "rollback") {
						yield* ValidateTransition(transaction, operation);
					}

					if (operation.lifecycle === "committed" || operation.lifecycle === "applied")
						return operation;

					const updated_at = yield* metadata.Now;
					const [updated] = yield* transaction
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "applied", updated_at })
						.where(
							and(
								eq(WorkspaceChangeOperations.message_id, input.message_id),
								eq(WorkspaceChangeOperations.lifecycle, "claimed"),
							),
						)
						.returning({ message_id: WorkspaceChangeOperations.message_id });

					if (!updated) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${input.message_id} changed before it was applied`,
						});
					}

					return yield* DecodeOperation({ ...row, lifecycle: "applied", updated_at });
				}),
			)
			.pipe(RetrySqliteWrite, Effect.mapError(normalize_error));

	const RejectChanged = (message_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [row] = yield* transaction
						.select()
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, message_id))
						.limit(1);

					if (!row)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} is missing`,
						});

					const operation = yield* DecodeOperation(row);

					yield* EnsureLiveThread(transaction, operation.thread_id);

					if (operation.action === "review")
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} cannot be rejected`,
						});

					if (operation.lifecycle === "rejected") {
						yield* ValidateRejectedState(transaction, operation);

						return operation;
					}

					if (operation.lifecycle !== "claimed")
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} cannot be rejected`,
						});

					yield* ValidateRejectedState(transaction, operation);

					const updated_at = yield* metadata.Now;
					const [updated] = yield* transaction
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "rejected", updated_at })
						.where(
							and(
								eq(WorkspaceChangeOperations.message_id, message_id),
								eq(WorkspaceChangeOperations.lifecycle, "claimed"),
							),
						)
						.returning({ message_id: WorkspaceChangeOperations.message_id });

					if (!updated) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} changed before it was rejected`,
						});
					}

					return yield* DecodeOperation({
						...row,
						lifecycle: "rejected",
						updated_at,
					});
				}),
			)
			.pipe(Effect.mapError(normalize_error));

	const ReconcileChanged = (input: ReconcileWorkspaceChange) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const Complete = (
						reconciliation: WorkspaceChangeReconciliation,
						journal_sequence?: number,
					) => ({ journal_sequence, reconciliation });
					const AppendConflictEvent = (
						action: WorkspaceConflictUpdatedEventValue["action"],
						conflict: WorkspaceConflictValue,
						occurred_at: string,
					) =>
						Effect.gen(function* () {
							const payload = yield* Schema.decodeUnknownEffect(
								WorkspaceConflictUpdatedEvent,
								{ onExcessProperty: "error" },
							)({ action, conflict, type: "workspace.conflict.updated" });
							const event = yield* AppendJournalEventInTransaction(
								transaction,
								{
									MakeId: metadata.MakeId,
									Now: Effect.succeed(occurred_at),
								},
								{
									...(operation.action === "replace"
										? {
												agent_id: operation.agent_id,
												run_id: operation.run_id,
											}
										: {}),
									...(operation.action === "replace" &&
									operation.raw_origin !== undefined
										? { raw_origin: operation.raw_origin }
										: {}),
									causation_id: operation.message_id,
									correlation_id: operation.message_id,
									payload,
									thread_id: operation.thread_id,
								},
							);
							return event.journal_sequence;
						});
					const message_id = input.message_id;
					const [row] = yield* transaction
						.select()
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, message_id))
						.limit(1);

					if (!row) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} is missing`,
						});
					}

					const operation = yield* DecodeOperation(row);

					yield* EnsureLiveThread(transaction, operation.thread_id);

					if (operation.action === "review") {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} cannot reconcile a file change`,
						});
					}

					if (operation.lifecycle === "committed") {
						const [reconciled] = yield* transaction
							.update(WorkspaceConflicts)
							.set({ resolution: "reconciled" })
							.where(
								and(
									eq(WorkspaceConflicts.source_command_id, operation.message_id),
									ne(WorkspaceConflicts.resolution, "reconciled"),
								),
							)
							.returning();
						const reconciliation_sequence = reconciled
							? yield* AppendConflictEvent(
									"updated",
									yield* DecodeConflict(reconciled),
									yield* metadata.Now,
								)
							: undefined;
						return Complete(
							{
								_tag: "committed" as const,
								event: yield* ReadDuplicate(transaction, operation),
								operation,
							},
							reconciliation_sequence,
						);
					}

					if (operation.lifecycle === "applied") {
						const [reconciled] = yield* transaction
							.update(WorkspaceConflicts)
							.set({ resolution: "reconciled" })
							.where(
								and(
									eq(WorkspaceConflicts.source_command_id, operation.message_id),
									ne(WorkspaceConflicts.resolution, "reconciled"),
								),
							)
							.returning();
						const reconciliation_sequence = reconciled
							? yield* AppendConflictEvent(
									"updated",
									yield* DecodeConflict(reconciled),
									yield* metadata.Now,
								)
							: undefined;
						return Complete(
							{ _tag: "applied" as const, operation },
							reconciliation_sequence,
						);
					}

					if (
						input.observation === "preflight_changed" &&
						(yield* HasAvailablePayload(transaction, operation))
					) {
						return Complete({ _tag: "staged" as const, operation });
					}

					const [source_change] = yield* transaction
						.select()
						.from(WorkspaceChanges)
						.where(eq(WorkspaceChanges.change_id, operation.change_id))
						.limit(1);
					const workspace_id =
						operation.action === "replace"
							? operation.workspace_id
							: source_change?.workspace_id;
					const path =
						operation.action === "replace" ? operation.path : source_change?.path;
					if (workspace_id === undefined || path === undefined) {
						return yield* new JournalInvariantError({
							message: `Workspace operation ${message_id} has no conflict attribution`,
						});
					}
					const authority_message_id =
						operation.action === "replace"
							? operation.message_id
							: source_change?.source_command_id;
					const [authority] =
						authority_message_id === undefined
							? []
							: yield* transaction
									.select()
									.from(WorkspaceMutationAuthorities)
									.where(
										eq(
											WorkspaceMutationAuthorities.message_id,
											authority_message_id,
										),
									)
									.limit(1);

					const projection_candidates = yield* transaction
						.select({
							after_identity_json: WorkspaceChanges.after_identity_json,
							change_id: WorkspaceChanges.change_id,
							updated_at: WorkspaceChanges.updated_at,
						})
						.from(WorkspaceChanges)
						.where(
							and(
								eq(WorkspaceChanges.workspace_id, workspace_id),
								eq(WorkspaceChanges.path, path),
								ne(WorkspaceChanges.change_id, operation.change_id),
							),
						)
						.orderBy(
							desc(WorkspaceChanges.updated_at),
							asc(WorkspaceChanges.change_id),
						);
					const FindCompetingProjection = (observed_identity: ContentIdentityValue) =>
						Effect.findFirst(projection_candidates, (candidate) =>
							candidate.updated_at >= row.created_at
								? DecodeJson(candidate.after_identity_json).pipe(
										Effect.flatMap(Schema.decodeUnknownEffect(ContentIdentity)),
										Effect.map((identity) =>
											identities_match(identity, observed_identity),
										),
									)
								: Effect.succeed(false),
						);
					const competing_projection = Option.getOrUndefined(
						input.observed_identity === undefined
							? Option.none()
							: yield* FindCompetingProjection(input.observed_identity),
					);
					const [competing_operation] =
						competing_projection !== undefined || input.observed_identity === undefined
							? []
							: yield* transaction
									.select({ change_id: WorkspaceChangeOperations.change_id })
									.from(WorkspaceChangeOperations)
									.where(
										and(
											eq(WorkspaceChangeOperations.action, "replace"),
											eq(
												WorkspaceChangeOperations.workspace_id,
												workspace_id,
											),
											eq(WorkspaceChangeOperations.path, path),
											eq(WorkspaceChangeOperations.lifecycle, "applied"),
											gte(
												WorkspaceChangeOperations.updated_at,
												row.created_at,
											),
											eq(
												WorkspaceChangeOperations.result_identity_json,
												JSON.stringify(input.observed_identity),
											),
											ne(
												WorkspaceChangeOperations.change_id,
												operation.change_id,
											),
										),
									)
									.orderBy(
										desc(WorkspaceChangeOperations.created_at),
										asc(WorkspaceChangeOperations.change_id),
									)
									.limit(1);
					const detected_at = yield* metadata.Now;
					const resolution = "user_action_required" as const;
					const conflict_row = {
						assignment_id: authority?.assignment_id ?? null,
						attempting_agent_id:
							operation.action === "replace"
								? operation.agent_id
								: (source_change?.agent_id ?? "unknown"),
						attempting_run_id:
							operation.action === "replace"
								? operation.run_id
								: (source_change?.run_id ?? "unknown"),
						attempting_thread_id: operation.thread_id,
						change_id: operation.change_id,
						competing_change_id:
							competing_projection?.change_id ??
							competing_operation?.change_id ??
							null,
						conflict_id: `conflict:${operation.message_id}`,
						detected_at,
						expected_identity_json: JSON.stringify(operation.expected_identity),
						group_id: authority?.group_id ?? null,
						observed_identity_json:
							input.observed_identity === undefined
								? null
								: JSON.stringify(input.observed_identity),
						path,
						raw_origin_json:
							operation.action === "replace" && operation.raw_origin !== undefined
								? JSON.stringify(operation.raw_origin)
								: null,
						resolution,
						source_command_id: operation.message_id,
						workspace_id,
					};
					let conflict_journal_sequence: number | undefined;
					if (
						input.observation === "native_changed" ||
						input.observed_identity !== undefined
					) {
						const [existing_conflict] = yield* transaction
							.select()
							.from(WorkspaceConflicts)
							.where(eq(WorkspaceConflicts.source_command_id, operation.message_id))
							.limit(1);
						if (existing_conflict) {
							yield* DecodeConflict(existing_conflict);
							if (
								existing_conflict.assignment_id !== conflict_row.assignment_id ||
								existing_conflict.attempting_agent_id !==
									conflict_row.attempting_agent_id ||
								existing_conflict.attempting_run_id !==
									conflict_row.attempting_run_id ||
								existing_conflict.attempting_thread_id !==
									conflict_row.attempting_thread_id ||
								existing_conflict.change_id !== conflict_row.change_id ||
								existing_conflict.conflict_id !== conflict_row.conflict_id ||
								existing_conflict.expected_identity_json !==
									conflict_row.expected_identity_json ||
								existing_conflict.group_id !== conflict_row.group_id ||
								existing_conflict.path !== conflict_row.path ||
								existing_conflict.raw_origin_json !==
									conflict_row.raw_origin_json ||
								existing_conflict.workspace_id !== conflict_row.workspace_id
							) {
								return yield* new JournalInvariantError({
									message: `Workspace conflict ${existing_conflict.conflict_id} changed immutable attribution`,
								});
							}
						}
						const materially_changed =
							!existing_conflict ||
							existing_conflict.competing_change_id !==
								conflict_row.competing_change_id ||
							existing_conflict.observed_identity_json !==
								conflict_row.observed_identity_json ||
							existing_conflict.resolution !== resolution;
						if (materially_changed) {
							const [written_conflict] = yield* transaction
								.insert(WorkspaceConflicts)
								.values(conflict_row)
								.onConflictDoUpdate({
									set: {
										competing_change_id: conflict_row.competing_change_id,
										observed_identity_json: conflict_row.observed_identity_json,
										resolution,
									},
									target: WorkspaceConflicts.source_command_id,
								})
								.returning();
							if (!written_conflict)
								return yield* new JournalInvariantError({
									message: "Workspace conflict was not persisted",
								});
							const conflict = yield* DecodeConflict(written_conflict);
							conflict_journal_sequence = yield* AppendConflictEvent(
								existing_conflict ? "updated" : "recorded",
								conflict,
								detected_at,
							);
						}
					}

					if (operation.lifecycle === "rejected") {
						yield* ValidateRejectedState(transaction, operation);

						return Complete(
							{ _tag: "rejected" as const, operation },
							conflict_journal_sequence,
						);
					}

					yield* ValidateRejectedState(transaction, operation);

					const updated_at = yield* metadata.Now;
					const [updated] = yield* transaction
						.update(WorkspaceChangeOperations)
						.set({ lifecycle: "rejected", updated_at })
						.where(
							and(
								eq(WorkspaceChangeOperations.message_id, message_id),
								eq(WorkspaceChangeOperations.lifecycle, "claimed"),
							),
						)
						.returning({ message_id: WorkspaceChangeOperations.message_id });

					if (!updated) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} changed before reconciliation`,
						});
					}

					return Complete(
						{
							_tag: "rejected" as const,
							operation: yield* DecodeOperation({
								...row,
								lifecycle: "rejected",
								updated_at,
							}),
						},
						conflict_journal_sequence,
					);
				}),
			)
			.pipe(
				RetrySqliteWrite,
				Effect.tap((result) =>
					result.journal_sequence === undefined
						? Effect.void
						: notifier.Publish(result.journal_sequence),
				),
				Effect.map((result) => result.reconciliation),
				Effect.mapError(normalize_error),
			);

	return { MarkApplied, ReconcileChanged, RejectChanged };
});
