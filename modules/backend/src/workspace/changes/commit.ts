import { and, eq } from "drizzle-orm";
import { Effect, Schema } from "effect";

import {
	type WorkspaceChange as WorkspaceChangeValue,
	type WorkspaceChangeUpdatedEvent as WorkspaceChangeUpdatedEventValue,
} from "@artisan/protocol";

import {
	JournalCommands,
	WorkspaceChangeOperations,
	WorkspaceChangeDiffs,
	WorkspaceChanges,
} from "../../persistence/tables";
import { JournalInvariantError } from "../../persistence/journal-store";
import { type PreparedWorkspaceChangeDiff } from "./diff";
import {
	WorkspaceChangeIdConflict,
	WorkspaceChangeJournalEvent,
	WorkspaceChangeTransitionError,
	type WorkspaceChangeOperation,
} from "./model";
import { DecodeChange, DecodeOperation } from "./storage-codec";

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

export const MakeCommit = Effect.gen(function* () {
	const context = yield* WorkspaceChangeContext;
	const {
		AppendJournalEventInTransaction,
		EnsureLiveThread,
		ReadDuplicate,
		ValidatePreparedDiff,
		ValidateTransition,
		command_payload_json,
		database,
		identities_match,
		metadata,
		normalize_commit_error,
		normalize_error,
		notifier,
	} = context;

	const AppendEvent = (
		transaction: typeof database.client,
		operation: WorkspaceChangeOperation,
		action: WorkspaceChangeUpdatedEventValue["action"],
		change: WorkspaceChangeValue,
		occurred_at: string,
	) =>
		Effect.gen(function* () {
			const payload = { action, change, type: "workspace.change.updated" } as const;
			const event = yield* AppendJournalEventInTransaction(
				transaction,
				{
					MakeId: metadata.MakeId,
					Now: Effect.succeed(occurred_at),
				},
				{
					...(action === "recorded" && operation.action === "replace"
						? {
								agent_id: operation.agent_id,
								run_id: operation.run_id,
							}
						: {}),
					...(action === "recorded" &&
					operation.action === "replace" &&
					operation.raw_origin !== undefined
						? { raw_origin: operation.raw_origin }
						: {}),
					causation_id: operation.message_id,
					correlation_id: operation.message_id,
					payload,
					thread_id: operation.thread_id,
				},
			);
			return yield* Schema.decodeUnknownEffect(WorkspaceChangeJournalEvent, {
				onExcessProperty: "error",
			})({
				causation_id: event.causation_id,
				correlation_id: event.correlation_id,
				event_id: event.message_id,
				journal_sequence: event.journal_sequence,
				occurred_at: event.sent_at,
				payload: event.payload,
				sequence: event.sequence,
			}).pipe(
				Effect.mapError(
					() =>
						new JournalInvariantError({
							message: `Stored workspace event ${event.message_id} is invalid`,
						}),
				),
			);
		});

	const Commit = (
		message_id: string,
		action: "recorded" | "reviewed" | "rolled_back",
		prepared_diff?: PreparedWorkspaceChangeDiff,
	) =>
		Effect.gen(function* () {
			const validated_diff =
				action === "recorded" ? yield* ValidatePreparedDiff(prepared_diff) : undefined;
			const result = yield* database.client.transaction((transaction) =>
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
					const expected_action =
						action === "recorded"
							? "replace"
							: action === "reviewed"
								? "review"
								: "rollback";

					yield* EnsureLiveThread(transaction, operation.thread_id);

					if (operation.action !== expected_action)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} has the wrong action`,
						});

					if (action === "recorded") {
						if (
							operation.action !== "replace" ||
							validated_diff === undefined ||
							validated_diff.message_id !== operation.message_id ||
							validated_diff.change_id !== operation.change_id ||
							validated_diff.thread_id !== operation.thread_id ||
							validated_diff.workspace_id !== operation.workspace_id ||
							validated_diff.path !== operation.path ||
							validated_diff.format_version !== operation.diff_format_version ||
							!identities_match(
								validated_diff.before_identity,
								operation.expected_identity,
							) ||
							!identities_match(
								validated_diff.after_identity,
								operation.result_identity,
							)
						) {
							return yield* new WorkspaceChangeTransitionError({
								message: "Workspace prepared diff is not bound to the operation",
							});
						}
					}

					if (operation.lifecycle === "committed") {
						return {
							event: yield* ReadDuplicate(transaction, operation, validated_diff),
							status: "duplicate" as const,
						};
					}

					if (
						(action === "recorded" || action === "rolled_back") &&
						operation.lifecycle !== "applied"
					)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} must be applied before commit`,
						});

					const now = yield* metadata.Now;
					let change: WorkspaceChangeValue;

					if (action === "recorded") {
						if (operation.action !== "replace")
							return yield* new JournalInvariantError({
								message: `Workspace replace ${message_id} has invalid action`,
							});

						const [existing_change] = yield* transaction
							.select()
							.from(WorkspaceChanges)
							.where(eq(WorkspaceChanges.change_id, operation.change_id))
							.limit(1);

						if (existing_change)
							return yield* new WorkspaceChangeIdConflict({
								change_id: operation.change_id,
							});

						if (validated_diff === undefined) {
							return yield* new WorkspaceChangeTransitionError({
								message: "Workspace replace requires a prepared diff",
							});
						}

						const inserted = {
							after_identity_json: JSON.stringify(operation.result_identity),
							agent_id: operation.agent_id,
							before_identity_json: JSON.stringify(operation.expected_identity),
							change_id: operation.change_id,
							created_at: now,
							path: operation.path,
							raw_origin_json:
								operation.raw_origin === undefined
									? null
									: JSON.stringify(operation.raw_origin),
							review_state: "needs_review",
							reviewed_at: null,
							review_source_command_id: null,
							reviewer_agent_id: null,
							reviewer_kind: null,
							reviewer_run_id: null,
							reviewer_assignment_id: null,
							reviewer_group_id: null,
							reviewer_raw_origin_json: null,
							review_outcome: null,
							review_comment: null,
							rollback_state: "available",
							rolled_back_at: null,
							run_id: operation.run_id,
							source_command_id: operation.message_id,
							thread_id: operation.thread_id,
							updated_at: now,
							version: 1,
							workspace_id: operation.workspace_id,
							diff_state: "available" as const,
						};
						yield* transaction.insert(WorkspaceChanges).values(inserted);
						yield* transaction.insert(WorkspaceChangeDiffs).values({
							added_line_count: validated_diff.added_line_count,
							after_identity_json: JSON.stringify(validated_diff.after_identity),
							before_identity_json: JSON.stringify(validated_diff.before_identity),
							change_id: validated_diff.change_id,
							context_lines: validated_diff.context_lines,
							created_at: now,
							format: validated_diff.format,
							format_version: validated_diff.format_version,
							patch: Buffer.from(validated_diff.patch),
							patch_byte_count: validated_diff.patch_identity.byte_count,
							patch_hash: validated_diff.patch_identity.content_hash,
							path: validated_diff.path,
							removed_line_count: validated_diff.removed_line_count,
							source_command_id: validated_diff.message_id,
							thread_id: validated_diff.thread_id,
							workspace_id: validated_diff.workspace_id,
						});
						change = yield* DecodeChange(inserted);
					} else {
						const transition = yield* ValidateTransition(transaction, operation);

						if (transition === undefined) {
							return yield* new JournalInvariantError({
								message: `Workspace transition ${message_id} has invalid action`,
							});
						}

						const stored = transition.row;
						if (action === "reviewed" && operation.action !== "review") {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace operation ${message_id} is not a review`,
							});
						}
						const review_operation =
							operation.action === "review" ? operation : undefined;
						const updated =
							action === "reviewed"
								? {
										...stored,
										review_state: "reviewed",
										reviewed_at: now,
										review_source_command_id: operation.message_id,
										reviewer_agent_id:
											review_operation?.reviewer_agent_id ?? null,
										reviewer_kind: review_operation?.reviewer_kind ?? null,
										reviewer_run_id: review_operation?.reviewer_run_id ?? null,
										reviewer_assignment_id:
											review_operation?.assignment_id ?? null,
										reviewer_group_id: review_operation?.group_id ?? null,
										reviewer_raw_origin_json:
											review_operation?.raw_origin === undefined
												? null
												: JSON.stringify(review_operation.raw_origin),
										review_outcome: review_operation?.outcome ?? null,
										review_comment: review_operation?.comment ?? null,
										updated_at: now,
										version: stored.version + 1,
									}
								: {
										...stored,
										review_state: "rolled_back",
										rollback_state: "consumed",
										rolled_back_at: now,
										updated_at: now,
										version: stored.version + 1,
									};
						const [written] = yield* transaction
							.update(WorkspaceChanges)
							.set(updated)
							.where(
								and(
									eq(WorkspaceChanges.change_id, operation.change_id),
									eq(WorkspaceChanges.thread_id, operation.thread_id),
									eq(WorkspaceChanges.version, stored.version),
									eq(WorkspaceChanges.review_state, stored.review_state),
									eq(WorkspaceChanges.rollback_state, stored.rollback_state),
								),
							)
							.returning({ change_id: WorkspaceChanges.change_id });

						if (!written) {
							return yield* new WorkspaceChangeTransitionError({
								message: `Workspace change ${operation.change_id} changed before commit`,
							});
						}

						change = yield* DecodeChange(updated);
					}

					const event = yield* AppendEvent(transaction, operation, action, change, now);
					yield* transaction.insert(JournalCommands).values({
						accepted_at: now,
						agent_id: operation.action === "replace" ? operation.agent_id : null,
						causation_id: null,
						message_id: operation.message_id,
						origin: "frontend",
						payload_json: command_payload_json(operation),
						payload_type: "workspace.change.command",
						raw_origin_json:
							operation.action === "replace" && operation.raw_origin !== undefined
								? JSON.stringify(operation.raw_origin)
								: null,
						run_id: operation.action === "replace" ? operation.run_id : null,
						schema_version: 1,
						sent_at: operation.sent_at,
						status: "accepted",
						thread_id: operation.thread_id,
					});
					const [committed] = yield* transaction
						.update(WorkspaceChangeOperations)
						.set({
							journal_sequence: event.journal_sequence,
							lifecycle: "committed",
							updated_at: now,
						})
						.where(
							and(
								eq(WorkspaceChangeOperations.message_id, message_id),
								eq(WorkspaceChangeOperations.lifecycle, operation.lifecycle),
							),
						)
						.returning({ message_id: WorkspaceChangeOperations.message_id });

					if (!committed) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} changed before commit`,
						});
					}

					return { event, status: "accepted" as const };
				}),
			);

			if (result.status === "accepted")
				yield* notifier.Publish(result.event.journal_sequence);

			return result;
		}).pipe(Effect.mapError(normalize_commit_error));

	const MarkEvidenceRecorded = (message_id: string) =>
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

					if (
						(operation.action !== "replace" && operation.action !== "rollback") ||
						operation.lifecycle !== "committed"
					)
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} cannot record evidence`,
						});

					yield* ReadDuplicate(transaction, operation);

					if (operation.evidence_recorded) return operation;
					const updated_at = yield* metadata.Now;
					const [updated] = yield* transaction
						.update(WorkspaceChangeOperations)
						.set({ evidence_recorded: true, updated_at })
						.where(
							and(
								eq(WorkspaceChangeOperations.message_id, message_id),
								eq(WorkspaceChangeOperations.lifecycle, "committed"),
								eq(WorkspaceChangeOperations.evidence_recorded, false),
							),
						)
						.returning({ message_id: WorkspaceChangeOperations.message_id });

					if (!updated) {
						return yield* new WorkspaceChangeTransitionError({
							message: `Workspace operation ${message_id} changed before evidence was recorded`,
						});
					}

					return yield* DecodeOperation({
						...row,
						evidence_recorded: true,
						updated_at,
					});
				}),
			)
			.pipe(Effect.mapError(normalize_error));

	return { Commit, MarkEvidenceRecorded };
});
