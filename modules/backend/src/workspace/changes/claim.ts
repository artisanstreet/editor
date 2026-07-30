import { and, eq, ne } from "drizzle-orm";
import { Effect, Schema } from "effect";

import { workspace_diff_format_version } from "@artisan/protocol";

import {
	AgentRuns,
	Assignments,
	OrchestrationGroups,
	JournalCommands,
	JournalEvents,
	WorkspaceChangeOperations,
} from "../../persistence/tables";
import { CommandIdConflict, JournalInvariantError } from "../../persistence/journal-store";
import {
	WorkspaceChangeIdConflict,
	WorkspaceChangeOperationSchema,
	WorkspaceChangeTransitionError,
	type ClaimReplace,
	type ClaimReview,
	type ClaimRollback,
} from "./model";
import { DecodeOperation } from "./storage-codec";

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

export const MakeClaim = Effect.gen(function* () {
	const context = yield* WorkspaceChangeContext;
	const {
		EnsureLiveThread,
		ReadDuplicate,
		ValidateRejectedState,
		ValidateTransition,
		database,
		immutable_operations_match,
		metadata,
		normalize_error,
		operation_from_claim,
	} = context;

	const Claim = (input: ClaimReplace | ClaimReview | ClaimRollback) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const claimed = operation_from_claim(input);
					const decoded_claim = yield* Schema.decodeUnknownEffect(
						WorkspaceChangeOperationSchema,
						{ onExcessProperty: "error" },
					)(claimed).pipe(
						Effect.mapError(
							() =>
								new JournalInvariantError({
									message: `Workspace claim ${input.message_id} is invalid`,
								}),
						),
					);

					yield* EnsureLiveThread(transaction, decoded_claim.thread_id);
					const [committed_retry] = yield* transaction
						.select()
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
						.limit(1);
					if (committed_retry?.lifecycle === "committed") {
						const operation = yield* DecodeOperation(committed_retry);
						if (!immutable_operations_match(operation, decoded_claim)) {
							return yield* new CommandIdConflict({
								message_id: input.message_id,
							});
						}
						return {
							_tag: "duplicate" as const,
							event: yield* ReadDuplicate(transaction, operation),
							operation,
						};
					}
					if (decoded_claim.action === "review") {
						const graph_fields = [
							decoded_claim.assignment_id,
							decoded_claim.group_id,
							decoded_claim.reviewer_agent_id,
							decoded_claim.reviewer_run_id,
						];
						if (decoded_claim.reviewer_kind === "user") {
							if (graph_fields.some((value) => value !== undefined))
								return yield* new WorkspaceChangeTransitionError({
									message: "User review cannot claim graph attribution",
								});
						} else {
							const assignment_id = decoded_claim.assignment_id;
							const group_id = decoded_claim.group_id;
							const reviewer_agent_id = decoded_claim.reviewer_agent_id;
							const reviewer_run_id = decoded_claim.reviewer_run_id;
							if (
								assignment_id === undefined ||
								group_id === undefined ||
								reviewer_agent_id === undefined ||
								reviewer_run_id === undefined
							)
								return yield* new WorkspaceChangeTransitionError({
									message: "Graph review attribution is incomplete",
								});
							const [run] = yield* transaction
								.select()
								.from(AgentRuns)
								.where(eq(AgentRuns.run_id, reviewer_run_id))
								.limit(1);
							const [assignment] = yield* transaction
								.select()
								.from(Assignments)
								.where(eq(Assignments.assignment_id, assignment_id))
								.limit(1);
							const [group] = yield* transaction
								.select()
								.from(OrchestrationGroups)
								.where(eq(OrchestrationGroups.group_id, group_id))
								.limit(1);
							if (
								!run ||
								!assignment ||
								!group ||
								run.agent_id !== decoded_claim.reviewer_agent_id ||
								run.assignment_id !== decoded_claim.assignment_id ||
								run.group_id !== decoded_claim.group_id ||
								assignment.group_id !== decoded_claim.group_id ||
								assignment.agent_id !== decoded_claim.reviewer_agent_id ||
								assignment.role !== "reviewer" ||
								assignment.state !== "active" ||
								assignment.active_run_id !== decoded_claim.reviewer_run_id ||
								assignment.current_attempt !== run.attempt ||
								run.state !== "running" ||
								run.dispatch_status !== "active" ||
								group.state !== "active" ||
								group.thread_id !== decoded_claim.thread_id
							)
								return yield* new WorkspaceChangeTransitionError({
									message: "Graph reviewer authority is invalid",
								});
						}
					}

					const [existing] = yield* transaction
						.select()
						.from(WorkspaceChangeOperations)
						.where(eq(WorkspaceChangeOperations.message_id, input.message_id))
						.limit(1);

					if (existing) {
						const operation = yield* DecodeOperation(existing);

						if (!immutable_operations_match(operation, decoded_claim)) {
							return yield* new CommandIdConflict({
								message_id: input.message_id,
							});
						}

						if (operation.lifecycle === "committed") {
							return {
								_tag: "duplicate" as const,
								event: yield* ReadDuplicate(transaction, operation),
								operation,
							};
						}

						if (operation.lifecycle === "rejected") {
							yield* ValidateRejectedState(transaction, operation);

							return { _tag: "rejected" as const, operation };
						}

						const [command] = yield* transaction
							.select({ message_id: JournalCommands.message_id })
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, operation.message_id))
							.limit(1);
						const [event] = yield* transaction
							.select({ event_id: JournalEvents.event_id })
							.from(JournalEvents)
							.where(
								and(
									eq(JournalEvents.correlation_id, operation.message_id),
									ne(JournalEvents.event_type, "workspace.conflict.updated"),
								),
							)
							.limit(1);

						if (command || event) {
							return yield* new JournalInvariantError({
								message: `Incomplete workspace operation ${operation.message_id} has committed journal state`,
							});
						}

						yield* ValidateTransition(transaction, operation);

						return { _tag: "incomplete_retry" as const, operation };
					}

					const [journal_command] = yield* transaction
						.select({ message_id: JournalCommands.message_id })
						.from(JournalCommands)
						.where(eq(JournalCommands.message_id, decoded_claim.message_id))
						.limit(1);

					if (journal_command) {
						return yield* new CommandIdConflict({ message_id: input.message_id });
					}

					const [orphaned_event] = yield* transaction
						.select({ event_id: JournalEvents.event_id })
						.from(JournalEvents)
						.where(eq(JournalEvents.correlation_id, decoded_claim.message_id))
						.limit(1);

					if (orphaned_event) {
						return yield* new JournalInvariantError({
							message: `Workspace message ${input.message_id} already owns an orphaned event`,
						});
					}

					const [claimed_change] = yield* transaction
						.select({ message_id: WorkspaceChangeOperations.message_id })
						.from(WorkspaceChangeOperations)
						.where(
							and(
								eq(WorkspaceChangeOperations.change_id, input.change_id),
								eq(WorkspaceChangeOperations.action, decoded_claim.action),
							),
						)
						.limit(1);

					if (claimed_change) {
						return yield* new WorkspaceChangeIdConflict({
							change_id: input.change_id,
						});
					}

					yield* ValidateTransition(transaction, decoded_claim);

					const now = yield* metadata.Now;
					const row = {
						action: decoded_claim.action,
						agent_id:
							decoded_claim.action === "replace" ? decoded_claim.agent_id : null,
						change_id: decoded_claim.change_id,
						created_at: now,
						diff_format_version: workspace_diff_format_version,
						evidence_recorded: false,
						expected_identity_json:
							decoded_claim.action === "review"
								? null
								: JSON.stringify(decoded_claim.expected_identity),
						journal_sequence: null,
						lifecycle: "claimed",
						message_id: decoded_claim.message_id,
						path: decoded_claim.action === "replace" ? decoded_claim.path : null,
						raw_origin_json:
							decoded_claim.action !== "rollback" &&
							decoded_claim.raw_origin !== undefined
								? JSON.stringify(decoded_claim.raw_origin)
								: null,
						reviewer_agent_id:
							decoded_claim.action === "review"
								? (decoded_claim.reviewer_agent_id ?? null)
								: null,
						reviewer_kind:
							decoded_claim.action === "review" ? decoded_claim.reviewer_kind : null,
						reviewer_run_id:
							decoded_claim.action === "review"
								? (decoded_claim.reviewer_run_id ?? null)
								: null,
						reviewer_assignment_id:
							decoded_claim.action === "review"
								? (decoded_claim.assignment_id ?? null)
								: null,
						reviewer_group_id:
							decoded_claim.action === "review"
								? (decoded_claim.group_id ?? null)
								: null,
						review_outcome:
							decoded_claim.action === "review"
								? (decoded_claim.outcome ?? null)
								: null,
						review_comment:
							decoded_claim.action === "review"
								? (decoded_claim.comment ?? null)
								: null,
						request_fingerprint: decoded_claim.request_fingerprint,
						result_identity_json:
							decoded_claim.action === "replace"
								? JSON.stringify(decoded_claim.result_identity)
								: null,
						run_id: decoded_claim.action === "replace" ? decoded_claim.run_id : null,
						sent_at: decoded_claim.sent_at,
						thread_id: decoded_claim.thread_id,
						updated_at: now,
						workspace_id:
							decoded_claim.action === "replace" ? decoded_claim.workspace_id : null,
					};

					yield* transaction.insert(WorkspaceChangeOperations).values(row);

					return { _tag: "claimed" as const, operation: yield* DecodeOperation(row) };
				}),
			)
			.pipe(Effect.mapError(normalize_error));

	return { Claim };
});
