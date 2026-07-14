import { and, asc, eq, isNull, notInArray, or } from "drizzle-orm";
import { Context, Data, DateTime, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	GitBranchName,
	GitObjectId,
	Identifier,
	IsoDateTime,
	RawOrigin,
	summarize_workspace_git_mutation,
	WorkspaceGitMutationApproval,
	WorkspaceGitMutationApprovalQuery,
	WorkspaceGitMutationApprovalQueryResult,
	WorkspaceGitMutationContinuationOperation,
	WorkspaceGitMutationOperation,
	WorkspaceGitMutationRejectionReason,
	WorkspaceGitMutationRequest,
	WorkspaceGitMutationSummary,
	WorkspaceGitMutationUnknownReason,
	type EventEnvelope as EventEnvelopeValue,
	type WorkspaceGitMutationApproval as WorkspaceGitMutationApprovalValue,
	type WorkspaceGitMutationApprovalQueryResult as WorkspaceGitMutationApprovalQueryResultValue,
	type WorkspaceGitMutationOperation as WorkspaceGitMutationOperationValue,
} from "@artisan/protocol";

import {
	git_mutation_plan_matches_operation,
	GitMutationAttempt,
	GitMutationPlan,
	GitMutationReconciliation,
	type GitMutationActionAnchor as GitMutationActionAnchorValue,
	type GitMutationAttempt as GitMutationAttemptValue,
	type GitMutationPlan as GitMutationPlanValue,
	type GitMutationReconciliation as GitMutationReconciliationValue,
} from "./git-mutation";
import { WorkspaceGitExecutionGate } from "./workspace-git-execution-gate";
import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStoreFailure } from "../persistence/journal-store";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
	WorkspaceChangeOperations,
	WorkspaceGitCheckoutClaims,
	WorkspaceGitMutationApprovals,
	WorkspaceGitMutationArtifacts,
	WorkspaceGitMutationClaims,
	WorkspaceGitSessions,
	WorkspaceMutationAuthorities,
} from "../persistence/schema";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));

const CommandMetadata = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	message_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sent_at: IsoDateTime,
});

const RequestMutation = Schema.Struct({
	action_approval_id: Schema.optional(Identifier),
	approval_id: Identifier,
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	operation: WorkspaceGitMutationOperation,
	plan: GitMutationPlan,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const ReplayMutationRequest = Schema.Struct({
	action_approval_id: Schema.optional(Identifier),
	approval_id: Identifier,
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	operation: WorkspaceGitMutationOperation,
	request_fingerprint: RequestFingerprint,
	source_command: CommandMetadata,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const MutationDecision = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	decision_command: CommandMetadata,
	thread_id: Identifier,
});

const ActionAnchorQuery = Schema.Struct({
	action_approval_id: Identifier,
	operation: WorkspaceGitMutationContinuationOperation,
	thread_id: Identifier,
	workspace_id: Identifier,
});

const ClaimIdentity = Schema.Struct({
	approval_id: Identifier,
	claim_token: Identifier,
});

const AppliedSettlement = Schema.Struct({
	...ClaimIdentity.fields,
	branch: Schema.optional(GitBranchName),
	head: GitObjectId,
	remote_head: Schema.optional(GitObjectId),
	type: Schema.Literal("applied"),
});

const ActionRequiredSettlement = Schema.Struct({
	...ClaimIdentity.fields,
	action: Schema.Literals(["merge_conflict", "rebase_conflict"]),
	type: Schema.Literal("action_required"),
});

const RejectedSettlement = Schema.Struct({
	...ClaimIdentity.fields,
	reason: WorkspaceGitMutationRejectionReason,
	type: Schema.Literal("rejected"),
});

const UnknownSettlement = Schema.Struct({
	...ClaimIdentity.fields,
	reason: WorkspaceGitMutationUnknownReason,
	type: Schema.Literal("outcome_unknown"),
});

const MutationSettlement = Schema.Union([
	AppliedSettlement,
	ActionRequiredSettlement,
	RejectedSettlement,
	UnknownSettlement,
]);

const RejectApprovedInput = Schema.Struct({
	approval_id: Identifier,
	reason: WorkspaceGitMutationRejectionReason,
});

const StoredMutationRequestPayload = Schema.Struct({
	action_approval_id: Schema.optional(Identifier),
	expected_session_version: Schema.Int.check(Schema.isGreaterThan(0)),
	operation: WorkspaceGitMutationSummary,
	request_fingerprint: Schema.optional(RequestFingerprint),
	type: Schema.Literal("workspace.git.mutation.request"),
	workspace_id: Identifier,
});

const StoredMutationDecisionPayload = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	type: Schema.Literal("workspace.git.mutation.approval.respond"),
});

export type RequestWorkspaceGitMutation = typeof RequestMutation.Type;
export type ReplayWorkspaceGitMutationRequest = typeof ReplayMutationRequest.Type;
export type WorkspaceGitMutationDecision = typeof MutationDecision.Type;
export type WorkspaceGitMutationSettlement = typeof MutationSettlement.Type;

export interface WorkspaceGitMutationAcceptance {
	readonly approval: WorkspaceGitMutationApprovalValue;
	readonly event: EventEnvelopeValue;
	readonly status: "accepted" | "duplicate";
}

export interface WorkspaceGitMutationExecution {
	readonly approval: WorkspaceGitMutationApprovalValue;
	readonly attempt?: GitMutationAttemptValue;
	readonly claim_token: string;
	readonly operation: WorkspaceGitMutationOperationValue;
	readonly plan: GitMutationPlanValue;
	readonly reconciliation?: GitMutationReconciliationValue;
}

export interface WorkspaceGitMutationDispatch {
	readonly approval_id: string;
	readonly recovery: "owned" | "quarantine" | "recoverable" | "waiting";
	readonly thread_id: string;
}

export class WorkspaceGitMutationConflict extends Data.TaggedError("WorkspaceGitMutationConflict")<{
	readonly reason:
		| "action_conflict"
		| "artifact_conflict"
		| "claim_conflict"
		| "command_conflict"
		| "decision_conflict"
		| "invalid_transition"
		| "lease_conflict"
		| "request_conflict"
		| "session_stale"
		| "workspace_mutation_active";
}> {}

export class WorkspaceGitMutationUnavailable extends Data.TaggedError(
	"WorkspaceGitMutationUnavailable",
)<{ readonly reason: "erased" | "missing" }> {}

export class WorkspaceGitMutationInvariant extends Data.TaggedError(
	"WorkspaceGitMutationInvariant",
)<{ readonly message: string }> {}

export type WorkspaceGitMutationRepositoryError =
	| JournalStoreFailure
	| WorkspaceGitMutationConflict
	| WorkspaceGitMutationInvariant
	| WorkspaceGitMutationUnavailable;

export class WorkspaceGitMutationRepository extends Context.Service<
	WorkspaceGitMutationRepository,
	{
		readonly AbandonOwnedExecutions: Effect.Effect<void, WorkspaceGitMutationRepositoryError>;
		readonly ClaimRecovery: (
			approval_id: string,
		) => Effect.Effect<
			Option.Option<WorkspaceGitMutationExecution>,
			WorkspaceGitMutationRepositoryError
		>;
		readonly Decide: (
			input: WorkspaceGitMutationDecision,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
		readonly ExecuteClaimed: <A, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, never, R>,
		) => Effect.Effect<A, WorkspaceGitMutationRepositoryError, R>;
		readonly ListApproved: Effect.Effect<
			ReadonlyArray<{ readonly approval_id: string; readonly thread_id: string }>,
			WorkspaceGitMutationRepositoryError
		>;
		readonly ListExecuting: Effect.Effect<
			ReadonlyArray<WorkspaceGitMutationDispatch>,
			WorkspaceGitMutationRepositoryError
		>;
		readonly MarkExecuting: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
		readonly Query: (
			query: typeof WorkspaceGitMutationApprovalQuery.Type,
		) => Effect.Effect<
			WorkspaceGitMutationApprovalQueryResultValue,
			WorkspaceGitMutationRepositoryError
		>;
		readonly QuarantineInterrupted: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
		readonly ReadActionAnchor: (
			input: typeof ActionAnchorQuery.Type,
		) => Effect.Effect<GitMutationActionAnchorValue, WorkspaceGitMutationRepositoryError>;
		readonly ReadBySourceCommand: (
			message_id: string,
		) => Effect.Effect<
			Option.Option<WorkspaceGitMutationAcceptance>,
			WorkspaceGitMutationRepositoryError
		>;
		readonly ReadExecution: (
			approval_id: string,
		) => Effect.Effect<WorkspaceGitMutationExecution, WorkspaceGitMutationRepositoryError>;
		readonly ReplayRequest: (
			input: ReplayWorkspaceGitMutationRequest,
		) => Effect.Effect<
			Option.Option<WorkspaceGitMutationAcceptance>,
			WorkspaceGitMutationRepositoryError
		>;
		readonly RecordAttempt: (
			identity: typeof ClaimIdentity.Type,
			attempt: unknown,
		) => Effect.Effect<void, WorkspaceGitMutationRepositoryError>;
		readonly RecordReconciliation: (
			identity: typeof ClaimIdentity.Type,
			reconciliation: unknown,
		) => Effect.Effect<void, WorkspaceGitMutationRepositoryError>;
		readonly RejectApproved: (
			input: typeof RejectApprovedInput.Type,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
		readonly Request: (
			input: RequestWorkspaceGitMutation,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
		readonly RenewLease: (
			identity: typeof ClaimIdentity.Type,
		) => Effect.Effect<void, WorkspaceGitMutationRepositoryError>;
		readonly Settle: (
			input: WorkspaceGitMutationSettlement,
		) => Effect.Effect<WorkspaceGitMutationAcceptance, WorkspaceGitMutationRepositoryError>;
	}
>()("Artisan/WorkspaceGitMutationRepository") {}

type ApprovalRow = typeof WorkspaceGitMutationApprovals.$inferSelect;
type ArtifactRow = typeof WorkspaceGitMutationArtifacts.$inferSelect;
type ClaimRow = typeof WorkspaceGitMutationClaims.$inferSelect;
type CommandRow = typeof JournalCommands.$inferSelect;

const execution_lease_seconds = 30;

interface DecodedArtifact {
	readonly attempt?: GitMutationAttemptValue;
	readonly operation: WorkspaceGitMutationOperationValue;
	readonly plan: GitMutationPlanValue;
	readonly reconciliation?: GitMutationReconciliationValue;
}

interface StoredRequestBinding {
	readonly acceptance: WorkspaceGitMutationAcceptance;
	readonly artifact: DecodedArtifact;
	readonly command_row: CommandRow;
	readonly row: ApprovalRow;
}

function invariant(message: string) {
	return new WorkspaceGitMutationInvariant({ message });
}

function DecodeDateTime(value: unknown, label: string) {
	return Schema.decodeUnknownEffect(IsoDateTime)(value).pipe(
		Effect.mapError(() => invariant(`${label} is not a valid timestamp`)),
		Effect.flatMap((decoded) =>
			Option.match(DateTime.make(decoded), {
				onNone: () => Effect.fail(invariant(`${label} is not a valid timestamp`)),
				onSome: Effect.succeed,
			}),
		),
	);
}

function LeaseExpiry(now: string) {
	return DecodeDateTime(now, "Git mutation lease clock").pipe(
		Effect.map((date_time) =>
			DateTime.formatIso(DateTime.add(date_time, { seconds: execution_lease_seconds })),
		),
	);
}

function LeaseExpired(expires_at: string, now: string) {
	return Effect.all([
		DecodeDateTime(expires_at, "Git mutation lease expiry"),
		DecodeDateTime(now, "Git mutation lease clock"),
	]).pipe(
		Effect.map(
			([expiry, current]) =>
				DateTime.toEpochMillis(expiry) <= DateTime.toEpochMillis(current),
		),
	);
}

function normalize_error(error: unknown): WorkspaceGitMutationRepositoryError {
	if (
		error instanceof WorkspaceGitMutationConflict ||
		error instanceof WorkspaceGitMutationInvariant ||
		error instanceof WorkspaceGitMutationUnavailable
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

function approval_event_key(
	approval_id: string,
	state: WorkspaceGitMutationApprovalValue["state"],
) {
	return `workspace_git_mutation:${approval_id}:${state}`;
}

function request_payload(input: ReplayWorkspaceGitMutationRequest) {
	return JSON.stringify({
		...(input.action_approval_id === undefined
			? {}
			: { action_approval_id: input.action_approval_id }),
		expected_session_version: input.expected_session_version,
		operation: summarize_workspace_git_mutation(input.operation),
		type: "workspace.git.mutation.request",
		workspace_id: input.workspace_id,
	});
}

function legacy_request_payload(input: ReplayWorkspaceGitMutationRequest) {
	return JSON.stringify({
		...(input.action_approval_id === undefined
			? {}
			: { action_approval_id: input.action_approval_id }),
		expected_session_version: input.expected_session_version,
		operation: summarize_workspace_git_mutation(input.operation),
		request_fingerprint: input.request_fingerprint,
		type: "workspace.git.mutation.request",
		workspace_id: input.workspace_id,
	});
}

function json_equals(left: unknown, right: unknown) {
	return JSON.stringify(left) === JSON.stringify(right);
}

function reconciliation_matches_private_evidence(
	plan: GitMutationPlanValue,
	attempt: GitMutationAttemptValue | undefined,
	reconciliation: GitMutationReconciliationValue,
) {
	if (reconciliation.type === "action_required") {
		return (
			(plan.type === "merge" || plan.type === "rebase") &&
			reconciliation.anchor.plan_binding === plan.binding &&
			reconciliation.anchor.type === plan.type &&
			reconciliation.action === (plan.type === "merge" ? "merge_conflict" : "rebase_conflict")
		);
	}

	if (reconciliation.type === "applied") {
		const common_evidence_matches =
			attempt !== undefined &&
			attempt.phase !== "precondition" &&
			attempt.output_complete &&
			attempt.rejection_reason === undefined &&
			attempt.result !== undefined &&
			attempt.result.branch === reconciliation.branch &&
			attempt.result.head === reconciliation.head &&
			attempt.result.state === "none";

		if (!common_evidence_matches) {
			return false;
		}

		if (plan.type === "push") {
			return (
				(attempt.exit_code === 0 || attempt.phase === "mutation") &&
				reconciliation.remote === plan.remote &&
				reconciliation.remote_endpoint === plan.remote_endpoint &&
				reconciliation.remote_head !== undefined &&
				reconciliation.target_branch === plan.target_branch &&
				attempt.operation_head === reconciliation.remote_head
			);
		}

		return (
			attempt.exit_code === 0 &&
			reconciliation.remote === undefined &&
			reconciliation.remote_endpoint === undefined &&
			reconciliation.remote_head === undefined &&
			reconciliation.target_branch === undefined
		);
	}

	if (reconciliation.type === "rejected") {
		return attempt?.rejection_reason === reconciliation.reason;
	}

	if (reconciliation.type === "source") {
		return attempt === undefined;
	}

	return true;
}

function decision_payload(input: WorkspaceGitMutationDecision) {
	return JSON.stringify({
		approval_id: input.approval_id,
		approved: input.approved,
		type: "workspace.git.mutation.approval.respond",
	});
}

function command_matches(
	row: CommandRow,
	metadata: typeof CommandMetadata.Type,
	thread_id: string,
	payload_type: string,
	payload_json: string,
) {
	return (
		row.message_id === metadata.message_id &&
		row.schema_version === 1 &&
		row.thread_id === thread_id &&
		row.run_id === (metadata.run_id ?? null) &&
		row.agent_id === (metadata.agent_id ?? null) &&
		row.causation_id === (metadata.causation_id ?? null) &&
		row.origin === "frontend" &&
		row.raw_origin_json ===
			(metadata.raw_origin === undefined ? null : JSON.stringify(metadata.raw_origin)) &&
		row.sent_at === metadata.sent_at &&
		row.payload_type === payload_type &&
		row.payload_json === payload_json &&
		row.status === "accepted"
	);
}

function request_matches_stored_binding(
	input: ReplayWorkspaceGitMutationRequest,
	binding: StoredRequestBinding,
) {
	const row = binding.row;

	return (
		row.approval_id === input.approval_id &&
		row.request_fingerprint === input.request_fingerprint &&
		row.thread_id === input.thread_id &&
		row.workspace_id === input.workspace_id &&
		row.expected_session_version === input.expected_session_version &&
		row.action_approval_id === (input.action_approval_id ?? null) &&
		json_equals(binding.artifact.operation, input.operation) &&
		(command_matches(
			binding.command_row,
			input.source_command,
			input.thread_id,
			"workspace.git.mutation.request",
			request_payload(input),
		) ||
			command_matches(
				binding.command_row,
				input.source_command,
				input.thread_id,
				"workspace.git.mutation.request",
				legacy_request_payload(input),
			))
	);
}

export const WorkspaceGitMutationRepositoryLive = Layer.effect(
	WorkspaceGitMutationRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const execution_gate = yield* WorkspaceGitExecutionGate;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
			Effect.gen(function* () {
				const [thread] = yield* transaction
					.select({ thread_id: Threads.thread_id })
					.from(Threads)
					.where(eq(Threads.thread_id, thread_id))
					.limit(1);
				const [claim] = yield* transaction
					.select({ thread_id: ThreadErasureClaims.thread_id })
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1);
				const [tombstone] = yield* transaction
					.select({ thread_id: ThreadTombstones.thread_id })
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1);

				if (!thread || claim || tombstone) {
					return yield* new WorkspaceGitMutationUnavailable({ reason: "erased" });
				}
			});

		const ReadRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(WorkspaceGitMutationApprovals)
				.where(eq(WorkspaceGitMutationApprovals.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									new WorkspaceGitMutationUnavailable({ reason: "missing" }),
								),
					),
				);

		const ReadArtifactRow = (transaction: typeof database.client, approval_id: string) =>
			transaction
				.select()
				.from(WorkspaceGitMutationArtifacts)
				.where(eq(WorkspaceGitMutationArtifacts.approval_id, approval_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? Effect.succeed(row)
							: Effect.fail(
									invariant(
										`Git mutation ${approval_id} has no private artifact`,
									),
								),
					),
				);

		const DecodeSummary = (row: ApprovalRow) =>
			Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
				row.operation_summary_json,
			).pipe(
				Effect.flatMap(
					Schema.decodeUnknownEffect(WorkspaceGitMutationSummary, {
						onExcessProperty: "error",
					}),
				),
				Effect.mapError(() =>
					invariant(`Git mutation ${row.approval_id} has an invalid summary`),
				),
			);

		const DecodeApprovalAtState = (
			row: ApprovalRow,
			state: WorkspaceGitMutationApprovalValue["state"],
		) =>
			Effect.gen(function* () {
				const operation = yield* DecodeSummary(row);
				const updated_at =
					state === "requested"
						? row.created_at
						: state === "approved" || state === "denied"
							? row.decided_at
							: state === "executing"
								? row.execution_started_at
								: row.updated_at;

				if (updated_at === null) {
					return yield* invariant(
						`Git mutation ${row.approval_id}:${state} has no update time`,
					);
				}

				const common = {
					...(row.action_approval_id === null
						? {}
						: { action_approval_id: row.action_approval_id }),
					approval_id: row.approval_id,
					created_at: row.created_at,
					expected_session_version: row.expected_session_version,
					operation,
					...(row.source_branch === null ? {} : { source_branch: row.source_branch }),
					source_command_id: row.source_command_id,
					source_head: row.source_head,
					thread_id: row.thread_id,
					updated_at,
					workspace_id: row.workspace_id,
				};
				const decision = {
					decided_at: row.decided_at,
					decision: "approved" as const,
					decision_message_id: row.decision_message_id,
				};
				const approval =
					state === "requested"
						? { ...common, state }
						: state === "denied"
							? {
									...common,
									decided_at: row.decided_at,
									decision: "denied" as const,
									decision_message_id: row.decision_message_id,
									state,
								}
							: state === "applied"
								? {
										...common,
										...decision,
										...(row.resulting_branch === null
											? {}
											: { resulting_branch: row.resulting_branch }),
										...(row.resulting_head === null
											? {}
											: { resulting_head: row.resulting_head }),
										...(row.remote_head === null
											? {}
											: { remote_head: row.remote_head }),
										state,
									}
								: state === "action_required"
									? {
											...common,
											...decision,
											action: row.required_action,
											state,
										}
									: state === "rejected"
										? {
												...common,
												...decision,
												reason: row.rejection_reason,
												state,
											}
										: state === "outcome_unknown"
											? {
													...common,
													...decision,
													reason: row.unknown_reason,
													state,
												}
											: { ...common, ...decision, state };

				return yield* Schema.decodeUnknownEffect(WorkspaceGitMutationApproval, {
					onExcessProperty: "error",
				})(approval).pipe(
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id}:${state} is corrupt`),
					),
				);
			});

		const DecodeApproval = (row: ApprovalRow) =>
			Effect.gen(function* () {
				const has_any_decision =
					row.decision_message_id !== null ||
					row.approved !== null ||
					row.decided_at !== null;
				const has_decision =
					row.decision_message_id !== null &&
					row.approved !== null &&
					row.decided_at !== null;
				const has_execution = row.execution_started_at !== null;
				const terminal = [
					"action_required",
					"applied",
					"outcome_unknown",
					"rejected",
				].includes(row.state);
				const valid_state =
					(row.state === "requested" && !has_any_decision && !has_execution) ||
					(row.state === "denied" &&
						has_decision &&
						row.approved === false &&
						!has_execution) ||
					(row.state === "approved" &&
						has_decision &&
						row.approved === true &&
						!has_execution) ||
					((row.state === "executing" || terminal) &&
						has_decision &&
						row.approved === true &&
						has_execution);
				const has_no_outcome =
					row.resulting_branch === null &&
					row.resulting_head === null &&
					row.remote_head === null &&
					row.required_action === null &&
					row.rejection_reason === null &&
					row.unknown_reason === null;
				const valid_outcome =
					(["requested", "approved", "executing", "denied"].includes(row.state) &&
						has_no_outcome) ||
					(row.state === "applied" &&
						row.resulting_head !== null &&
						row.required_action === null &&
						row.rejection_reason === null &&
						row.unknown_reason === null) ||
					(row.state === "action_required" &&
						row.required_action !== null &&
						row.resulting_branch === null &&
						row.resulting_head === null &&
						row.remote_head === null &&
						row.rejection_reason === null &&
						row.unknown_reason === null) ||
					(row.state === "rejected" &&
						row.rejection_reason !== null &&
						row.resulting_branch === null &&
						row.resulting_head === null &&
						row.remote_head === null &&
						row.required_action === null &&
						row.unknown_reason === null) ||
					(row.state === "outcome_unknown" &&
						row.unknown_reason !== null &&
						row.resulting_branch === null &&
						row.resulting_head === null &&
						row.remote_head === null &&
						row.required_action === null &&
						row.rejection_reason === null);
				const expected_updated_at =
					row.state === "requested"
						? row.created_at
						: row.state === "approved" || row.state === "denied"
							? row.decided_at
							: row.state === "executing"
								? row.execution_started_at
								: row.updated_at;

				if (
					!valid_state ||
					!valid_outcome ||
					expected_updated_at === null ||
					row.updated_at !== expected_updated_at
				) {
					return yield* invariant(`Git mutation ${row.approval_id} has an invalid state`);
				}

				yield* Schema.decodeUnknownEffect(RequestFingerprint)(row.request_fingerprint).pipe(
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id} has an invalid fingerprint`),
					),
				);
				yield* Schema.decodeUnknownEffect(GitObjectId)(row.source_head).pipe(
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id} has an invalid source head`),
					),
				);

				if (row.source_branch !== null) {
					yield* Schema.decodeUnknownEffect(GitBranchName)(row.source_branch).pipe(
						Effect.mapError(() =>
							invariant(
								`Git mutation ${row.approval_id} has an invalid source branch`,
							),
						),
					);
				}

				return yield* DecodeApprovalAtState(
					row,
					row.state as WorkspaceGitMutationApprovalValue["state"],
				);
			});

		const DecodeArtifact = (row: ArtifactRow) =>
			Effect.gen(function* () {
				const operation = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.operation_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(WorkspaceGitMutationOperation, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id} has an invalid operation`),
					),
				);
				const plan = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.plan_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(GitMutationPlan, { onExcessProperty: "error" }),
					),
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id} has an invalid plan`),
					),
				);

				if (
					plan.binding !== row.plan_binding ||
					!git_mutation_plan_matches_operation(plan, operation)
				) {
					return yield* invariant(
						`Git mutation ${row.approval_id} plan binding is corrupt`,
					);
				}

				const has_any_attempt = row.attempt_json !== null || row.attempt_binding !== null;
				const has_attempt = row.attempt_json !== null && row.attempt_binding !== null;

				if (has_any_attempt !== has_attempt) {
					return yield* invariant(
						`Git mutation ${row.approval_id} attempt pair is corrupt`,
					);
				}

				const attempt = has_attempt
					? yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
							row.attempt_json,
						).pipe(
							Effect.flatMap(
								Schema.decodeUnknownEffect(GitMutationAttempt, {
									onExcessProperty: "error",
								}),
							),
							Effect.mapError(() =>
								invariant(`Git mutation ${row.approval_id} has an invalid attempt`),
							),
						)
					: undefined;

				if (
					attempt !== undefined &&
					(attempt.binding !== row.attempt_binding ||
						attempt.plan_binding !== plan.binding)
				) {
					return yield* invariant(
						`Git mutation ${row.approval_id} attempt binding is corrupt`,
					);
				}

				const has_any_reconciliation =
					row.reconciliation_json !== null || row.reconciled_at !== null;
				const has_reconciliation =
					row.reconciliation_json !== null && row.reconciled_at !== null;

				if (has_any_reconciliation !== has_reconciliation) {
					return yield* invariant(
						`Git mutation ${row.approval_id} reconciliation pair is corrupt`,
					);
				}

				const reconciliation = has_reconciliation
					? yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
							row.reconciliation_json,
						).pipe(
							Effect.flatMap(
								Schema.decodeUnknownEffect(GitMutationReconciliation, {
									onExcessProperty: "error",
								}),
							),
							Effect.mapError(() =>
								invariant(
									`Git mutation ${row.approval_id} has an invalid reconciliation`,
								),
							),
						)
					: undefined;

				if (
					reconciliation !== undefined &&
					!reconciliation_matches_private_evidence(plan, attempt, reconciliation)
				) {
					return yield* invariant(
						`Git mutation ${row.approval_id} reconciliation evidence is corrupt`,
					);
				}

				if (row.reconciled_at !== null) {
					yield* Schema.decodeUnknownEffect(IsoDateTime)(row.reconciled_at).pipe(
						Effect.mapError(() =>
							invariant(
								`Git mutation ${row.approval_id} has an invalid reconciliation time`,
							),
						),
					);
				}

				return {
					...(attempt === undefined ? {} : { attempt }),
					operation,
					plan,
					...(reconciliation === undefined ? {} : { reconciliation }),
				};
			});

		const ReadArtifact = (transaction: typeof database.client, approval_id: string) =>
			ReadArtifactRow(transaction, approval_id).pipe(Effect.flatMap(DecodeArtifact));

		const DecodeEventRow = (row: typeof JournalEvents.$inferSelect) =>
			Effect.gen(function* () {
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.mapError(() =>
						invariant("Stored Git mutation event payload is corrupt"),
					),
				);

				return yield* Schema.decodeUnknownEffect(EventEnvelope, {
					onExcessProperty: "error",
				})({
					causation_id: row.causation_id,
					correlation_id: row.correlation_id,
					journal_sequence: row.sequence,
					kind: "event",
					message_id: row.event_id,
					origin: row.origin,
					payload,
					protocol_version: 1,
					schema_version: row.schema_version,
					sent_at: row.occurred_at,
					sequence: row.stream_sequence,
					stream_id: row.stream_id,
					thread_id: row.thread_id,
				}).pipe(Effect.mapError(() => invariant("Stored Git mutation event is corrupt")));
			});

		const ReadEvent = (
			transaction: typeof database.client,
			approval_id: string,
			state: WorkspaceGitMutationApprovalValue["state"],
		) =>
			transaction
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.idempotency_key, approval_event_key(approval_id, state)))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						row
							? DecodeEventRow(row)
							: Effect.fail(
									invariant(
										`Git mutation ${approval_id}:${state} event is missing`,
									),
								),
					),
				);

		const ReadAcceptance = (
			transaction: typeof database.client,
			row: ApprovalRow,
			state: WorkspaceGitMutationApprovalValue["state"],
		) =>
			Effect.gen(function* () {
				yield* DecodeApproval(row);

				const approval = yield* DecodeApprovalAtState(row, state);
				const event = yield* ReadEvent(transaction, row.approval_id, state);
				const is_request = state === "requested";
				const is_decision = state === "approved" || state === "denied";
				const expected_causation_id =
					is_request || is_decision ? row.source_command_id : row.decision_message_id;
				const expected_correlation_id = is_request
					? row.approval_id
					: is_decision
						? row.decision_message_id
						: row.approval_id;

				if (
					event.payload.type !== "workspace.git.mutation.approval.updated" ||
					expected_causation_id === null ||
					expected_correlation_id === null ||
					!json_equals(event.payload.approval, approval) ||
					event.causation_id !== expected_causation_id ||
					event.correlation_id !== expected_correlation_id ||
					event.origin !== "backend" ||
					event.sent_at !== approval.updated_at ||
					event.stream_id !== `thread:${approval.thread_id}` ||
					event.thread_id !== approval.thread_id
				) {
					return yield* invariant(`Git mutation ${row.approval_id}:${state} is corrupt`);
				}

				return { approval, event };
			});

		const AppendEvent = (
			transaction: typeof database.client,
			approval: WorkspaceGitMutationApprovalValue,
			causation_id: string,
			correlation_id: string,
		) =>
			Effect.gen(function* () {
				const stream_id = `thread:${approval.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const stream_sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const payload = {
					approval,
					type: "workspace.git.mutation.approval.updated",
				} as const;

				if (stream) {
					yield* transaction
						.update(EventStreams)
						.set({ last_sequence: stream_sequence })
						.where(eq(EventStreams.stream_id, stream_id));
				} else {
					yield* transaction
						.insert(EventStreams)
						.values({ last_sequence: stream_sequence, stream_id });
				}

				const [row] = yield* transaction
					.insert(JournalEvents)
					.values({
						causation_id,
						correlation_id,
						event_id,
						event_type: payload.type,
						idempotency_key: approval_event_key(approval.approval_id, approval.state),
						occurred_at: approval.updated_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						schema_version: 1,
						stream_id,
						stream_sequence,
						thread_id: approval.thread_id,
					})
					.returning();

				if (!row) {
					return yield* invariant("Git mutation event was not persisted");
				}

				return yield* DecodeEventRow(row);
			});

		const InsertCommand = (
			transaction: typeof database.client,
			command: typeof CommandMetadata.Type,
			thread_id: string,
			payload_type: string,
			payload_json: string,
		) =>
			Effect.gen(function* () {
				const accepted_at = yield* metadata.Now;

				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: "frontend",
					payload_json,
					payload_type,
					raw_origin_json:
						command.raw_origin === undefined
							? null
							: JSON.stringify(command.raw_origin),
					run_id: command.run_id ?? null,
					schema_version: 1,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id,
				});
			});

		const DecodeStoredCommandMetadata = (
			row: CommandRow,
			payload_type: string,
			label: string,
		) =>
			Effect.gen(function* () {
				if (
					row.schema_version !== 1 ||
					row.origin !== "frontend" ||
					row.payload_type !== payload_type ||
					row.status !== "accepted" ||
					row.assigned_run_id !== null
				) {
					return yield* invariant(`${label} command ${row.message_id} is corrupt`);
				}

				yield* Schema.decodeUnknownEffect(IsoDateTime)(row.accepted_at).pipe(
					Effect.mapError(() =>
						invariant(`${label} command ${row.message_id} has invalid acceptance time`),
					),
				);

				const raw_origin =
					row.raw_origin_json === null
						? undefined
						: yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
								row.raw_origin_json,
							).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(RawOrigin, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(() =>
									invariant(
										`${label} command ${row.message_id} has invalid origin`,
									),
								),
							);
				const value = {
					...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
					...(row.causation_id === null ? {} : { causation_id: row.causation_id }),
					message_id: row.message_id,
					...(raw_origin === undefined ? {} : { raw_origin }),
					...(row.run_id === null ? {} : { run_id: row.run_id }),
					sent_at: row.sent_at,
				};

				return yield* Schema.decodeUnknownEffect(CommandMetadata, {
					onExcessProperty: "error",
				})(value).pipe(
					Effect.mapError(() =>
						invariant(`${label} command ${row.message_id} has invalid metadata`),
					),
				);
			});

		const DecodeStoredRequestCommand = (row: CommandRow) =>
			Effect.gen(function* () {
				const command = yield* DecodeStoredCommandMetadata(
					row,
					"workspace.git.mutation.request",
					"Git mutation request",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredMutationRequestPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Git mutation request ${row.message_id} has invalid payload`),
					),
				);

				return { command, payload };
			});

		const DecodeStoredDecisionCommand = (row: CommandRow) =>
			Effect.gen(function* () {
				const command = yield* DecodeStoredCommandMetadata(
					row,
					"workspace.git.mutation.approval.respond",
					"Git mutation decision",
				);
				const payload = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
					row.payload_json,
				).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(StoredMutationDecisionPayload, {
							onExcessProperty: "error",
						}),
					),
					Effect.mapError(() =>
						invariant(`Git mutation decision ${row.message_id} has invalid payload`),
					),
				);

				return { command, payload };
			});

		const ReadStoredRequestBinding = (
			transaction: typeof database.client,
			message_id: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(WorkspaceGitMutationApprovals)
					.where(eq(WorkspaceGitMutationApprovals.source_command_id, message_id))
					.limit(1);
				const [command_row] = yield* transaction
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, message_id))
					.limit(1);

				if (!row) {
					if (command_row?.payload_type === "workspace.git.mutation.request") {
						return yield* invariant(
							`Git mutation request ${message_id} has no approval`,
						);
					}

					return Option.none<StoredRequestBinding>();
				}

				yield* EnsureLiveThread(transaction, row.thread_id);

				if (!command_row) {
					return yield* invariant(
						`Git mutation ${row.approval_id} has no source command`,
					);
				}

				const stored = yield* DecodeStoredRequestCommand(command_row);
				const approval = yield* DecodeApproval(row);
				const artifact = yield* ReadArtifact(transaction, row.approval_id);
				const summary = summarize_workspace_git_mutation(artifact.operation);

				if (
					command_row.thread_id !== row.thread_id ||
					stored.command.sent_at !== row.created_at ||
					(stored.payload.request_fingerprint !== undefined &&
						stored.payload.request_fingerprint !== row.request_fingerprint) ||
					stored.payload.workspace_id !== row.workspace_id ||
					stored.payload.expected_session_version !== row.expected_session_version ||
					stored.payload.action_approval_id !== (row.action_approval_id ?? undefined) ||
					!json_equals(stored.payload.operation, summary) ||
					!json_equals(approval.operation, summary) ||
					approval.source_command_id !== command_row.message_id ||
					artifact.plan.source.head !== row.source_head ||
					artifact.plan.source.branch !== (row.source_branch ?? undefined)
				) {
					return yield* invariant(
						`Git mutation ${row.approval_id} has an invalid request binding`,
					);
				}

				const acceptance = yield* ReadAcceptance(transaction, row, "requested");

				const binding: StoredRequestBinding = {
					acceptance: { ...acceptance, status: "duplicate" as const },
					artifact,
					command_row,
					row,
				};

				return Option.some(binding);
			});

		const ReadSessionSource = (
			transaction: typeof database.client,
			workspace_id: string,
			expected_version: number,
			expected_branch: string | undefined,
			expected_head: string,
		) =>
			Effect.gen(function* () {
				const [row] = yield* transaction
					.select()
					.from(WorkspaceGitSessions)
					.where(eq(WorkspaceGitSessions.workspace_id, workspace_id))
					.limit(1);

				if (
					!row ||
					row.version !== expected_version ||
					row.state === "unavailable" ||
					row.repository_root === null ||
					row.selected_worktree_path === null ||
					row.branch !== (expected_branch ?? null) ||
					row.head !== expected_head
				) {
					return yield* new WorkspaceGitMutationConflict({ reason: "session_stale" });
				}

				yield* Schema.decodeUnknownEffect(GitObjectId)(row.head).pipe(
					Effect.mapError(() => invariant("Git mutation source session head is corrupt")),
				);

				if (row.branch !== null) {
					yield* Schema.decodeUnknownEffect(GitBranchName)(row.branch).pipe(
						Effect.mapError(() =>
							invariant("Git mutation source session branch is corrupt"),
						),
					);
				}
			});

		const ReadActionAnchorInternal = (
			transaction: typeof database.client,
			input: typeof ActionAnchorQuery.Type,
		) =>
			Effect.gen(function* () {
				const row = yield* ReadRow(transaction, input.action_approval_id);

				yield* EnsureLiveThread(transaction, row.thread_id);

				const expected_action =
					input.operation.type === "merge" ? "merge_conflict" : "rebase_conflict";

				if (
					row.thread_id !== input.thread_id ||
					row.workspace_id !== input.workspace_id ||
					row.state !== "action_required" ||
					row.required_action !== expected_action
				) {
					return yield* new WorkspaceGitMutationConflict({ reason: "action_conflict" });
				}

				const artifact = yield* ReadArtifact(transaction, row.approval_id);
				const reconciliation = artifact.reconciliation;

				if (
					reconciliation?.type !== "action_required" ||
					reconciliation.action !== expected_action ||
					reconciliation.anchor.type !== input.operation.type
				) {
					return yield* invariant(
						`Git mutation ${row.approval_id} has an invalid action anchor`,
					);
				}

				return reconciliation.anchor;
			});

		const EnsureActionParentAvailable = (
			transaction: typeof database.client,
			input: typeof ActionAnchorQuery.Type,
		) =>
			Effect.gen(function* () {
				const anchor = yield* ReadActionAnchorInternal(transaction, input);
				const [child] = yield* transaction
					.select({ approval_id: WorkspaceGitMutationApprovals.approval_id })
					.from(WorkspaceGitMutationApprovals)
					.where(
						and(
							eq(
								WorkspaceGitMutationApprovals.action_approval_id,
								input.action_approval_id,
							),
							notInArray(WorkspaceGitMutationApprovals.state, ["denied", "rejected"]),
						),
					)
					.limit(1);

				if (child) {
					return yield* new WorkspaceGitMutationConflict({ reason: "action_conflict" });
				}

				return anchor;
			});

		const DecodeRequest = (input: RequestWorkspaceGitMutation) =>
			Schema.decodeUnknownEffect(RequestMutation, { onExcessProperty: "error" })(input).pipe(
				Effect.flatMap((decoded) =>
					Schema.decodeUnknownEffect(WorkspaceGitMutationRequest, {
						onExcessProperty: "error",
					})({
						...(decoded.action_approval_id === undefined
							? {}
							: { action_approval_id: decoded.action_approval_id }),
						expected_session_version: decoded.expected_session_version,
						operation: decoded.operation,
						workspace_id: decoded.workspace_id,
					}).pipe(Effect.as(decoded)),
				),
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "request_conflict" }),
				),
			);
		const DecodeReplayRequest = (input: ReplayWorkspaceGitMutationRequest) =>
			Schema.decodeUnknownEffect(ReplayMutationRequest, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.flatMap((decoded) =>
					Schema.decodeUnknownEffect(WorkspaceGitMutationRequest, {
						onExcessProperty: "error",
					})({
						...(decoded.action_approval_id === undefined
							? {}
							: { action_approval_id: decoded.action_approval_id }),
						expected_session_version: decoded.expected_session_version,
						operation: decoded.operation,
						workspace_id: decoded.workspace_id,
					}).pipe(Effect.as(decoded)),
				),
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "request_conflict" }),
				),
			);
		const ReplayRequest = (input: ReplayWorkspaceGitMutationRequest) =>
			DecodeReplayRequest(input).pipe(
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const stored = yield* ReadStoredRequestBinding(
								transaction,
								decoded.source_command.message_id,
							);

							if (Option.isSome(stored)) {
								if (!request_matches_stored_binding(decoded, stored.value)) {
									return yield* new WorkspaceGitMutationConflict({
										reason: "request_conflict",
									});
								}

								return Option.some(stored.value.acceptance);
							}

							const [command_collision] = yield* transaction
								.select({ message_id: JournalCommands.message_id })
								.from(JournalCommands)
								.where(
									eq(
										JournalCommands.message_id,
										decoded.source_command.message_id,
									),
								)
								.limit(1);

							if (command_collision) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "command_conflict",
								});
							}

							return Option.none<WorkspaceGitMutationAcceptance>();
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const Request = (input: RequestWorkspaceGitMutation) =>
			DecodeRequest(input).pipe(
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const stored = yield* ReadStoredRequestBinding(
										transaction,
										decoded.source_command.message_id,
									);

									if (Option.isSome(stored)) {
										const binding = stored.value;

										if (!request_matches_stored_binding(decoded, binding)) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "request_conflict",
											});
										}

										return binding.acceptance;
									}

									yield* EnsureLiveThread(transaction, decoded.thread_id);

									const [approval_collision] = yield* transaction
										.select({
											approval_id: WorkspaceGitMutationApprovals.approval_id,
										})
										.from(WorkspaceGitMutationApprovals)
										.where(
											eq(
												WorkspaceGitMutationApprovals.approval_id,
												decoded.approval_id,
											),
										)
										.limit(1);
									const [command_collision] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.source_command.message_id,
											),
										)
										.limit(1);

									if (approval_collision) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "request_conflict",
										});
									}

									if (command_collision) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "command_conflict",
										});
									}

									if (
										!git_mutation_plan_matches_operation(
											decoded.plan,
											decoded.operation,
										)
									) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "artifact_conflict",
										});
									}

									if (decoded.action_approval_id !== undefined) {
										const operation = yield* Schema.decodeUnknownEffect(
											WorkspaceGitMutationContinuationOperation,
											{ onExcessProperty: "error" },
										)(decoded.operation).pipe(
											Effect.mapError(
												() =>
													new WorkspaceGitMutationConflict({
														reason: "action_conflict",
													}),
											),
										);
										const anchor = yield* EnsureActionParentAvailable(
											transaction,
											{
												action_approval_id: decoded.action_approval_id,
												operation,
												thread_id: decoded.thread_id,
												workspace_id: decoded.workspace_id,
											},
										);

										if (
											(decoded.plan.type !== "merge" &&
												decoded.plan.type !== "rebase") ||
											decoded.plan.action === "start" ||
											!json_equals(decoded.plan.anchor, anchor)
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "action_conflict",
											});
										}
									}

									yield* ReadSessionSource(
										transaction,
										decoded.workspace_id,
										decoded.expected_session_version,
										decoded.plan.source.branch,
										decoded.plan.source.head,
									);

									const operation_summary = summarize_workspace_git_mutation(
										decoded.operation,
									);

									yield* InsertCommand(
										transaction,
										decoded.source_command,
										decoded.thread_id,
										"workspace.git.mutation.request",
										request_payload(decoded),
									);
									yield* transaction
										.insert(WorkspaceGitMutationApprovals)
										.values({
											action_approval_id: decoded.action_approval_id ?? null,
											approval_id: decoded.approval_id,
											created_at: decoded.source_command.sent_at,
											expected_session_version:
												decoded.expected_session_version,
											operation_summary_json:
												JSON.stringify(operation_summary),
											request_fingerprint: decoded.request_fingerprint,
											source_branch: decoded.plan.source.branch ?? null,
											source_command_id: decoded.source_command.message_id,
											source_head: decoded.plan.source.head,
											state: "requested",
											thread_id: decoded.thread_id,
											updated_at: decoded.source_command.sent_at,
											workspace_id: decoded.workspace_id,
										});
									yield* transaction
										.insert(WorkspaceGitMutationArtifacts)
										.values({
											approval_id: decoded.approval_id,
											operation_json: JSON.stringify(decoded.operation),
											plan_binding: decoded.plan.binding,
											plan_json: JSON.stringify(decoded.plan),
											updated_at: decoded.source_command.sent_at,
										});

									const row = yield* ReadRow(transaction, decoded.approval_id);
									const approval = yield* DecodeApproval(row);
									const event = yield* AppendEvent(
										transaction,
										approval,
										decoded.source_command.message_id,
										decoded.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const Decide = (input: WorkspaceGitMutationDecision) =>
			Schema.decodeUnknownEffect(MutationDecision, { onExcessProperty: "error" })(input).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "decision_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.thread_id !== decoded.thread_id) {
										return yield* new WorkspaceGitMutationUnavailable({
											reason: "missing",
										});
									}

									if (row.decision_message_id !== null) {
										if (
											row.decision_message_id !==
												decoded.decision_command.message_id ||
											row.approved !== decoded.approved ||
											row.decided_at !== decoded.decision_command.sent_at
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "decision_conflict",
											});
										}

										const [command] = yield* transaction
											.select()
											.from(JournalCommands)
											.where(
												eq(
													JournalCommands.message_id,
													row.decision_message_id,
												),
											)
											.limit(1);

										if (!command) {
											return yield* invariant(
												`Git mutation ${row.approval_id} has no decision command`,
											);
										}

										const stored_command =
											yield* DecodeStoredDecisionCommand(command);
										const expected_state = decoded.approved
											? "approved"
											: "denied";

										if (
											stored_command.command.sent_at !== row.decided_at ||
											stored_command.payload.approval_id !==
												row.approval_id ||
											stored_command.payload.approved !== row.approved ||
											!command_matches(
												command,
												decoded.decision_command,
												decoded.thread_id,
												"workspace.git.mutation.approval.respond",
												decision_payload(decoded),
											)
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "decision_conflict",
											});
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											expected_state,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "requested") {
										return yield* new WorkspaceGitMutationConflict({
											reason: "invalid_transition",
										});
									}

									const [collision] = yield* transaction
										.select({ message_id: JournalCommands.message_id })
										.from(JournalCommands)
										.where(
											eq(
												JournalCommands.message_id,
												decoded.decision_command.message_id,
											),
										)
										.limit(1);

									if (collision) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "command_conflict",
										});
									}

									yield* InsertCommand(
										transaction,
										decoded.decision_command,
										decoded.thread_id,
										"workspace.git.mutation.approval.respond",
										decision_payload(decoded),
									);

									const target_state = decoded.approved ? "approved" : "denied";
									const [updated] = yield* transaction
										.update(WorkspaceGitMutationApprovals)
										.set({
											approved: decoded.approved,
											decided_at: decoded.decision_command.sent_at,
											decision_message_id:
												decoded.decision_command.message_id,
											state: target_state,
											updated_at: decoded.decision_command.sent_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitMutationApprovals.approval_id,
													row.approval_id,
												),
												eq(
													WorkspaceGitMutationApprovals.state,
													"requested",
												),
											),
										)
										.returning();

									if (!updated) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "invalid_transition",
										});
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										row.source_command_id,
										decoded.decision_command.message_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const ReadClaim = (
			transaction: typeof database.client,
			row: ApprovalRow,
			claim_token?: string,
			owner_instance_id?: string,
		) =>
			Effect.gen(function* () {
				const [claim] = yield* transaction
					.select()
					.from(WorkspaceGitMutationClaims)
					.where(eq(WorkspaceGitMutationClaims.approval_id, row.approval_id))
					.limit(1);

				if (
					!claim ||
					(claim_token !== undefined && claim.claim_token !== claim_token) ||
					(owner_instance_id !== undefined &&
						claim.owner_instance_id !== owner_instance_id)
				) {
					return yield* new WorkspaceGitMutationConflict({ reason: "lease_conflict" });
				}

				yield* Schema.decodeUnknownEffect(Identifier)(claim.owner_instance_id).pipe(
					Effect.mapError(() =>
						invariant(`Git mutation ${row.approval_id} has an invalid lease owner`),
					),
				);
				yield* DecodeDateTime(
					claim.lease_expires_at,
					`Git mutation ${row.approval_id} lease expiry`,
				);

				if (claim.execution_started_at !== null) {
					yield* DecodeDateTime(
						claim.execution_started_at,
						`Git mutation ${row.approval_id} execution start`,
					);
				}

				if (claim.execution_completed_at !== null) {
					yield* DecodeDateTime(
						claim.execution_completed_at,
						`Git mutation ${row.approval_id} execution completion`,
					);
				}

				if (
					row.state !== "executing" ||
					row.execution_started_at === null ||
					(claim.execution_completed_at !== null &&
						claim.execution_started_at === null) ||
					claim.workspace_id !== row.workspace_id ||
					claim.thread_id !== row.thread_id ||
					claim.claimed_at !== row.execution_started_at
				) {
					return yield* invariant(`Git mutation ${row.approval_id} claim is corrupt`);
				}

				return claim;
			});
		const BuildExecution = (
			transaction: typeof database.client,
			row: ApprovalRow,
			claim: ClaimRow,
		) =>
			Effect.gen(function* () {
				const artifact = yield* ReadArtifact(transaction, row.approval_id);

				return {
					approval: yield* DecodeApproval(row),
					...(artifact.attempt === undefined ? {} : { attempt: artifact.attempt }),
					claim_token: claim.claim_token,
					operation: artifact.operation,
					plan: artifact.plan,
					...(artifact.reconciliation === undefined
						? {}
						: { reconciliation: artifact.reconciliation }),
				};
			});

		const MarkExecuting = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === "executing") {
										yield* ReadClaim(transaction, row);

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											"executing",
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "approved") {
										return yield* new WorkspaceGitMutationConflict({
											reason: "invalid_transition",
										});
									}

									const artifact = yield* ReadArtifact(
										transaction,
										row.approval_id,
									);

									yield* ReadSessionSource(
										transaction,
										row.workspace_id,
										row.expected_session_version,
										artifact.plan.source.branch,
										artifact.plan.source.head,
									);

									const [mutation] = yield* transaction
										.select({
											message_id: WorkspaceChangeOperations.message_id,
										})
										.from(WorkspaceChangeOperations)
										.innerJoin(
											WorkspaceMutationAuthorities,
											eq(
												WorkspaceMutationAuthorities.message_id,
												WorkspaceChangeOperations.message_id,
											),
										)
										.where(
											and(
												eq(
													WorkspaceMutationAuthorities.workspace_id,
													row.workspace_id,
												),
												notInArray(WorkspaceChangeOperations.lifecycle, [
													"committed",
													"rejected",
												]),
											),
										)
										.limit(1);

									if (mutation) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "workspace_mutation_active",
										});
									}

									const [checkout_claim] = yield* transaction
										.select({
											approval_id: WorkspaceGitCheckoutClaims.approval_id,
										})
										.from(WorkspaceGitCheckoutClaims)
										.where(
											eq(
												WorkspaceGitCheckoutClaims.workspace_id,
												row.workspace_id,
											),
										)
										.limit(1);

									if (checkout_claim) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "claim_conflict",
										});
									}

									const [existing_claim] = yield* transaction
										.select()
										.from(WorkspaceGitMutationClaims)
										.where(
											or(
												eq(
													WorkspaceGitMutationClaims.workspace_id,
													row.workspace_id,
												),
												eq(
													WorkspaceGitMutationClaims.approval_id,
													row.approval_id,
												),
											),
										)
										.limit(1);

									if (existing_claim) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "claim_conflict",
										});
									}

									const started_at = yield* metadata.Now;
									const lease_expires_at = yield* LeaseExpiry(started_at);
									const claim_token = yield* metadata.MakeId("claim");
									const [claim] = yield* transaction
										.insert(WorkspaceGitMutationClaims)
										.values({
											approval_id: row.approval_id,
											claimed_at: started_at,
											claim_token,
											lease_expires_at,
											owner_instance_id: metadata.instance_id,
											thread_id: row.thread_id,
											workspace_id: row.workspace_id,
										})
										.onConflictDoNothing()
										.returning();

									if (!claim) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "claim_conflict",
										});
									}

									const [updated] = yield* transaction
										.update(WorkspaceGitMutationApprovals)
										.set({
											execution_started_at: started_at,
											state: "executing",
											updated_at: started_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitMutationApprovals.approval_id,
													row.approval_id,
												),
												eq(WorkspaceGitMutationApprovals.state, "approved"),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* invariant(
											"Git mutation execution transition did not persist",
										);
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const ReadExecution = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded);

							yield* EnsureLiveThread(transaction, row.thread_id);

							if (row.state !== "executing") {
								return yield* new WorkspaceGitMutationConflict({
									reason: "invalid_transition",
								});
							}

							const claim = yield* ReadClaim(
								transaction,
								row,
								undefined,
								metadata.instance_id,
							);

							return yield* BuildExecution(transaction, row, claim);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);
		const RenewLease = (identity: typeof ClaimIdentity.Type) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "lease_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const lease_expires_at = yield* LeaseExpiry(now);

						yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									const claim = yield* ReadClaim(
										transaction,
										row,
										decoded.claim_token,
										metadata.instance_id,
									);
									const [renewed] = yield* transaction
										.update(WorkspaceGitMutationClaims)
										.set({ lease_expires_at })
										.where(
											and(
												eq(
													WorkspaceGitMutationClaims.approval_id,
													decoded.approval_id,
												),
												eq(
													WorkspaceGitMutationClaims.claim_token,
													decoded.claim_token,
												),
												eq(
													WorkspaceGitMutationClaims.owner_instance_id,
													metadata.instance_id,
												),
												eq(
													WorkspaceGitMutationClaims.lease_expires_at,
													claim.lease_expires_at,
												),
											),
										)
										.returning({
											claim_token: WorkspaceGitMutationClaims.claim_token,
										});

									if (!renewed) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "lease_conflict",
										});
									}
								}),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);
		const MarkExecutionStarted = (identity: typeof ClaimIdentity.Type) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const row = yield* ReadRow(transaction, identity.approval_id);

						yield* EnsureLiveThread(transaction, row.thread_id);

						const claim = yield* ReadClaim(
							transaction,
							row,
							identity.claim_token,
							metadata.instance_id,
						);

						if (
							claim.execution_started_at !== null ||
							claim.execution_completed_at !== null
						) {
							return yield* new WorkspaceGitMutationConflict({
								reason: "lease_conflict",
							});
						}

						const execution_started_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceGitMutationClaims)
							.set({ execution_started_at })
							.where(
								and(
									eq(
										WorkspaceGitMutationClaims.approval_id,
										identity.approval_id,
									),
									eq(
										WorkspaceGitMutationClaims.claim_token,
										identity.claim_token,
									),
									eq(
										WorkspaceGitMutationClaims.owner_instance_id,
										metadata.instance_id,
									),
									isNull(WorkspaceGitMutationClaims.execution_started_at),
									isNull(WorkspaceGitMutationClaims.execution_completed_at),
								),
							)
							.returning({
								approval_id: WorkspaceGitMutationClaims.approval_id,
							});

						if (!updated) {
							return yield* new WorkspaceGitMutationConflict({
								reason: "lease_conflict",
							});
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));
		const MarkExecutionCompleted = (identity: typeof ClaimIdentity.Type) =>
			RetrySqliteWrite(
				database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const row = yield* ReadRow(transaction, identity.approval_id);

						yield* EnsureLiveThread(transaction, row.thread_id);

						const claim = yield* ReadClaim(
							transaction,
							row,
							identity.claim_token,
							metadata.instance_id,
						);

						if (
							claim.execution_started_at === null ||
							claim.execution_completed_at !== null
						) {
							return yield* new WorkspaceGitMutationConflict({
								reason: "lease_conflict",
							});
						}

						const execution_completed_at = yield* metadata.Now;
						const [updated] = yield* transaction
							.update(WorkspaceGitMutationClaims)
							.set({ execution_completed_at })
							.where(
								and(
									eq(
										WorkspaceGitMutationClaims.approval_id,
										identity.approval_id,
									),
									eq(
										WorkspaceGitMutationClaims.claim_token,
										identity.claim_token,
									),
									eq(
										WorkspaceGitMutationClaims.owner_instance_id,
										metadata.instance_id,
									),
									eq(
										WorkspaceGitMutationClaims.execution_started_at,
										claim.execution_started_at,
									),
									isNull(WorkspaceGitMutationClaims.execution_completed_at),
								),
							)
							.returning({
								approval_id: WorkspaceGitMutationClaims.approval_id,
							});

						if (!updated) {
							return yield* new WorkspaceGitMutationConflict({
								reason: "lease_conflict",
							});
						}
					}),
				),
			).pipe(Effect.mapError(normalize_error));
		const ExecuteClaimed = <A, R>(
			identity: typeof ClaimIdentity.Type,
			execution: Effect.Effect<A, never, R>,
		) =>
			Schema.decodeUnknownEffect(ClaimIdentity, { onExcessProperty: "error" })(identity).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "lease_conflict" }),
				),
				Effect.flatMap((decoded) =>
					execution_gate.Run(
						decoded.approval_id,
						decoded.claim_token,
						Effect.gen(function* () {
							yield* RenewLease(decoded);
							yield* MarkExecutionStarted(decoded);

							const result = yield* execution.pipe(
								Effect.onExit(() => MarkExecutionCompleted(decoded)),
							);

							yield* RenewLease(decoded);

							return result;
						}),
					),
				),
				Effect.mapError(normalize_error),
			);
		const ClaimRecovery = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const now = yield* metadata.Now;
						const lease_expires_at = yield* LeaseExpiry(now);

						return yield* execution_gate.Run(
							decoded,
							metadata.instance_id,
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const [row] = yield* transaction
											.select()
											.from(WorkspaceGitMutationApprovals)
											.where(
												eq(
													WorkspaceGitMutationApprovals.approval_id,
													decoded,
												),
											)
											.limit(1);

										if (!row || row.state !== "executing") {
											return Option.none<WorkspaceGitMutationExecution>();
										}

										yield* EnsureLiveThread(transaction, row.thread_id);

										const claim = yield* ReadClaim(transaction, row);
										const expired = yield* LeaseExpired(
											claim.lease_expires_at,
											now,
										);

										if (!expired) {
											return Option.none<WorkspaceGitMutationExecution>();
										}

										if (
											claim.execution_started_at !== null &&
											claim.execution_completed_at === null
										) {
											return Option.none<WorkspaceGitMutationExecution>();
										}

										const claim_token = yield* metadata.MakeId("claim");
										const [recovered] = yield* transaction
											.update(WorkspaceGitMutationClaims)
											.set({
												claim_token,
												lease_expires_at,
												owner_instance_id: metadata.instance_id,
											})
											.where(
												and(
													eq(
														WorkspaceGitMutationClaims.approval_id,
														decoded,
													),
													eq(
														WorkspaceGitMutationClaims.claim_token,
														claim.claim_token,
													),
													eq(
														WorkspaceGitMutationClaims.owner_instance_id,
														claim.owner_instance_id,
													),
													eq(
														WorkspaceGitMutationClaims.lease_expires_at,
														claim.lease_expires_at,
													),
												),
											)
											.returning();

										if (!recovered) {
											return Option.none<WorkspaceGitMutationExecution>();
										}

										return Option.some(
											yield* BuildExecution(transaction, row, recovered),
										);
									}),
								),
							),
						);
					}),
				),
				Effect.mapError(normalize_error),
			);
		const QuarantineInterrupted = (approval_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(approval_id).pipe(
				Effect.mapError(() => new WorkspaceGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* execution_gate.Run(
							decoded,
							metadata.instance_id,
							RetrySqliteWrite(
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const row = yield* ReadRow(transaction, decoded);

										yield* EnsureLiveThread(transaction, row.thread_id);

										if (
											row.state === "outcome_unknown" &&
											row.unknown_reason === "interrupted"
										) {
											const acceptance = yield* ReadAcceptance(
												transaction,
												row,
												"outcome_unknown",
											);

											return { ...acceptance, status: "duplicate" as const };
										}

										if (row.state !== "executing") {
											return yield* new WorkspaceGitMutationConflict({
												reason: "invalid_transition",
											});
										}

										const claim = yield* ReadClaim(transaction, row);
										const now = yield* metadata.Now;
										const expired = yield* LeaseExpired(
											claim.lease_expires_at,
											now,
										);

										if (
											!expired ||
											claim.execution_started_at === null ||
											claim.execution_completed_at !== null
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "lease_conflict",
											});
										}

										const artifact_row = yield* ReadArtifactRow(
											transaction,
											row.approval_id,
										);
										const artifact = yield* DecodeArtifact(artifact_row);

										if (
											artifact.reconciliation !== undefined &&
											artifact.reconciliation.type !== "outcome_unknown"
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "artifact_conflict",
											});
										}

										if (artifact.reconciliation === undefined) {
											const [updated_artifact] = yield* transaction
												.update(WorkspaceGitMutationArtifacts)
												.set({
													reconciled_at: now,
													reconciliation_json: JSON.stringify({
														type: "outcome_unknown",
													}),
													updated_at: now,
												})
												.where(
													and(
														eq(
															WorkspaceGitMutationArtifacts.approval_id,
															row.approval_id,
														),
														isNull(
															WorkspaceGitMutationArtifacts.reconciliation_json,
														),
													),
												)
												.returning({
													approval_id:
														WorkspaceGitMutationArtifacts.approval_id,
												});

											if (!updated_artifact) {
												return yield* new WorkspaceGitMutationConflict({
													reason: "artifact_conflict",
												});
											}
										}

										const [updated] = yield* transaction
											.update(WorkspaceGitMutationApprovals)
											.set({
												state: "outcome_unknown",
												unknown_reason: "interrupted",
												updated_at: now,
											})
											.where(
												and(
													eq(
														WorkspaceGitMutationApprovals.approval_id,
														row.approval_id,
													),
													eq(
														WorkspaceGitMutationApprovals.state,
														"executing",
													),
												),
											)
											.returning();

										if (!updated || updated.decision_message_id === null) {
											return yield* invariant(
												"Git mutation quarantine transition did not persist",
											);
										}

										const approval = yield* DecodeApproval(updated);
										const event = yield* AppendEvent(
											transaction,
											approval,
											updated.decision_message_id,
											updated.approval_id,
										);

										return { approval, event, status: "accepted" as const };
									}),
								),
							),
						);

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
				Effect.mapError(normalize_error),
			);
		const AbandonOwnedExecutions = Effect.gen(function* () {
			const now = yield* metadata.Now;

			yield* DecodeDateTime(now, "Git mutation lease clock");
			yield* RetrySqliteWrite(
				database.client
					.update(WorkspaceGitMutationClaims)
					.set({ lease_expires_at: now })
					.where(eq(WorkspaceGitMutationClaims.owner_instance_id, metadata.instance_id)),
			);
		}).pipe(Effect.mapError(normalize_error));

		const RecordAttempt = (identity: typeof ClaimIdentity.Type, attempt: unknown) =>
			Effect.gen(function* () {
				const decoded_identity = yield* Schema.decodeUnknownEffect(ClaimIdentity, {
					onExcessProperty: "error",
				})(identity).pipe(
					Effect.mapError(
						() => new WorkspaceGitMutationConflict({ reason: "lease_conflict" }),
					),
				);
				const decoded_attempt = yield* Schema.decodeUnknownEffect(GitMutationAttempt, {
					onExcessProperty: "error",
				})(attempt).pipe(
					Effect.mapError(
						() => new WorkspaceGitMutationConflict({ reason: "artifact_conflict" }),
					),
				);

				yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded_identity.approval_id);

							yield* EnsureLiveThread(transaction, row.thread_id);
							yield* ReadClaim(
								transaction,
								row,
								decoded_identity.claim_token,
								metadata.instance_id,
							);

							const artifact_row = yield* ReadArtifactRow(
								transaction,
								row.approval_id,
							);
							const artifact = yield* DecodeArtifact(artifact_row);

							if (decoded_attempt.plan_binding !== artifact.plan.binding) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "artifact_conflict",
								});
							}

							if (artifact.attempt !== undefined) {
								if (!json_equals(artifact.attempt, decoded_attempt)) {
									return yield* new WorkspaceGitMutationConflict({
										reason: "artifact_conflict",
									});
								}

								return;
							}

							if (artifact.reconciliation !== undefined) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "invalid_transition",
								});
							}

							const updated_at = yield* metadata.Now;
							const [updated] = yield* transaction
								.update(WorkspaceGitMutationArtifacts)
								.set({
									attempt_binding: decoded_attempt.binding,
									attempt_json: JSON.stringify(decoded_attempt),
									updated_at,
								})
								.where(
									and(
										eq(
											WorkspaceGitMutationArtifacts.approval_id,
											row.approval_id,
										),
										isNull(WorkspaceGitMutationArtifacts.attempt_json),
									),
								)
								.returning({
									approval_id: WorkspaceGitMutationArtifacts.approval_id,
								});

							if (!updated) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "artifact_conflict",
								});
							}
						}),
					),
				).pipe(Effect.mapError(normalize_error));
			});

		const RecordReconciliation = (
			identity: typeof ClaimIdentity.Type,
			reconciliation: unknown,
		) =>
			Effect.gen(function* () {
				const decoded_identity = yield* Schema.decodeUnknownEffect(ClaimIdentity, {
					onExcessProperty: "error",
				})(identity).pipe(
					Effect.mapError(
						() => new WorkspaceGitMutationConflict({ reason: "lease_conflict" }),
					),
				);
				const decoded_reconciliation = yield* Schema.decodeUnknownEffect(
					GitMutationReconciliation,
					{ onExcessProperty: "error" },
				)(reconciliation).pipe(
					Effect.mapError(
						() => new WorkspaceGitMutationConflict({ reason: "artifact_conflict" }),
					),
				);

				yield* RetrySqliteWrite(
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const row = yield* ReadRow(transaction, decoded_identity.approval_id);

							yield* EnsureLiveThread(transaction, row.thread_id);
							yield* ReadClaim(
								transaction,
								row,
								decoded_identity.claim_token,
								metadata.instance_id,
							);

							const artifact_row = yield* ReadArtifactRow(
								transaction,
								row.approval_id,
							);
							const artifact = yield* DecodeArtifact(artifact_row);
							const invalid_reconciliation = !reconciliation_matches_private_evidence(
								artifact.plan,
								artifact.attempt,
								decoded_reconciliation,
							);

							if (invalid_reconciliation) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "artifact_conflict",
								});
							}

							if (artifact.reconciliation !== undefined) {
								if (!json_equals(artifact.reconciliation, decoded_reconciliation)) {
									return yield* new WorkspaceGitMutationConflict({
										reason: "artifact_conflict",
									});
								}

								return;
							}

							const reconciled_at = yield* metadata.Now;
							const [updated] = yield* transaction
								.update(WorkspaceGitMutationArtifacts)
								.set({
									reconciled_at,
									reconciliation_json: JSON.stringify(decoded_reconciliation),
									updated_at: reconciled_at,
								})
								.where(
									and(
										eq(
											WorkspaceGitMutationArtifacts.approval_id,
											row.approval_id,
										),
										isNull(WorkspaceGitMutationArtifacts.reconciliation_json),
									),
								)
								.returning({
									approval_id: WorkspaceGitMutationArtifacts.approval_id,
								});

							if (!updated) {
								return yield* new WorkspaceGitMutationConflict({
									reason: "artifact_conflict",
								});
							}
						}),
					),
				).pipe(Effect.mapError(normalize_error));
			});

		const SettlementMatches = (
			reconciliation: GitMutationReconciliationValue,
			settlement: WorkspaceGitMutationSettlement,
		) => {
			if (settlement.type === "applied") {
				return (
					reconciliation.type === "applied" &&
					reconciliation.branch === settlement.branch &&
					reconciliation.head === settlement.head &&
					reconciliation.remote_head === settlement.remote_head
				);
			}

			if (settlement.type === "action_required") {
				return (
					reconciliation.type === "action_required" &&
					reconciliation.action === settlement.action
				);
			}

			if (settlement.type === "rejected") {
				return (
					reconciliation.type === "rejected" &&
					reconciliation.reason === settlement.reason
				);
			}

			return (
				reconciliation.type === "outcome_unknown" ||
				(reconciliation.type === "source" && settlement.reason === "interrupted")
			);
		};

		const TerminalState = (settlement: WorkspaceGitMutationSettlement) =>
			settlement.type === "outcome_unknown" ? "outcome_unknown" : settlement.type;

		const TerminalRowMatches = (
			row: ApprovalRow,
			settlement: WorkspaceGitMutationSettlement,
		) => {
			if (settlement.type === "applied") {
				return (
					row.resulting_branch === (settlement.branch ?? null) &&
					row.resulting_head === settlement.head &&
					row.remote_head === (settlement.remote_head ?? null)
				);
			}

			if (settlement.type === "action_required") {
				return row.required_action === settlement.action;
			}

			if (settlement.type === "rejected") {
				return row.rejection_reason === settlement.reason;
			}

			return row.unknown_reason === settlement.reason;
		};

		const Settle = (input: WorkspaceGitMutationSettlement) =>
			Schema.decodeUnknownEffect(MutationSettlement, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "artifact_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);
									const target_state = TerminalState(decoded);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === target_state) {
										const [claim] = yield* transaction
											.select({
												approval_id: WorkspaceGitMutationClaims.approval_id,
											})
											.from(WorkspaceGitMutationClaims)
											.where(
												eq(
													WorkspaceGitMutationClaims.approval_id,
													row.approval_id,
												),
											)
											.limit(1);
										const artifact = yield* ReadArtifact(
											transaction,
											row.approval_id,
										);

										if (
											claim ||
											artifact.reconciliation === undefined ||
											!SettlementMatches(artifact.reconciliation, decoded) ||
											!TerminalRowMatches(row, decoded)
										) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "artifact_conflict",
											});
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											target_state,
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "executing") {
										return yield* new WorkspaceGitMutationConflict({
											reason: "invalid_transition",
										});
									}

									yield* ReadClaim(
										transaction,
										row,
										decoded.claim_token,
										metadata.instance_id,
									);

									const artifact = yield* ReadArtifact(
										transaction,
										row.approval_id,
									);

									if (
										artifact.reconciliation === undefined ||
										!SettlementMatches(artifact.reconciliation, decoded)
									) {
										return yield* new WorkspaceGitMutationConflict({
											reason: "artifact_conflict",
										});
									}

									const updated_at = yield* metadata.Now;
									const values =
										decoded.type === "applied"
											? {
													resulting_branch: decoded.branch ?? null,
													resulting_head: decoded.head,
													remote_head: decoded.remote_head ?? null,
													state: target_state,
													updated_at,
												}
											: decoded.type === "action_required"
												? {
														required_action: decoded.action,
														state: target_state,
														updated_at,
													}
												: decoded.type === "rejected"
													? {
															rejection_reason: decoded.reason,
															state: target_state,
															updated_at,
														}
													: {
															unknown_reason: decoded.reason,
															state: target_state,
															updated_at,
														};
									const [updated] = yield* transaction
										.update(WorkspaceGitMutationApprovals)
										.set(values)
										.where(
											and(
												eq(
													WorkspaceGitMutationApprovals.approval_id,
													row.approval_id,
												),
												eq(
													WorkspaceGitMutationApprovals.state,
													"executing",
												),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* invariant(
											"Git mutation terminal transition did not persist",
										);
									}

									const [released] = yield* transaction
										.delete(WorkspaceGitMutationClaims)
										.where(
											and(
												eq(
													WorkspaceGitMutationClaims.approval_id,
													row.approval_id,
												),
												eq(
													WorkspaceGitMutationClaims.claim_token,
													decoded.claim_token,
												),
											),
										)
										.returning({
											approval_id: WorkspaceGitMutationClaims.approval_id,
										});

									if (!released) {
										return yield* invariant(
											"Git mutation claim release did not persist",
										);
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const RejectApproved = (input: typeof RejectApprovedInput.Type) =>
			Schema.decodeUnknownEffect(RejectApprovedInput, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "request_conflict" }),
				),
				Effect.flatMap((decoded) =>
					Effect.gen(function* () {
						const result = yield* RetrySqliteWrite(
							database.client.transaction((transaction) =>
								Effect.gen(function* () {
									const row = yield* ReadRow(transaction, decoded.approval_id);

									yield* EnsureLiveThread(transaction, row.thread_id);

									if (row.state === "rejected") {
										const [claim] = yield* transaction
											.select({
												approval_id: WorkspaceGitMutationClaims.approval_id,
											})
											.from(WorkspaceGitMutationClaims)
											.where(
												eq(
													WorkspaceGitMutationClaims.approval_id,
													row.approval_id,
												),
											)
											.limit(1);

										if (claim || row.rejection_reason !== decoded.reason) {
											return yield* new WorkspaceGitMutationConflict({
												reason: "invalid_transition",
											});
										}

										const acceptance = yield* ReadAcceptance(
											transaction,
											row,
											"rejected",
										);

										return { ...acceptance, status: "duplicate" as const };
									}

									if (row.state !== "approved") {
										return yield* new WorkspaceGitMutationConflict({
											reason: "invalid_transition",
										});
									}

									const [claim] = yield* transaction
										.select({
											approval_id: WorkspaceGitMutationClaims.approval_id,
										})
										.from(WorkspaceGitMutationClaims)
										.where(
											eq(
												WorkspaceGitMutationClaims.approval_id,
												row.approval_id,
											),
										)
										.limit(1);

									if (claim) {
										return yield* invariant(
											"Approved Git mutation retained a claim",
										);
									}

									const updated_at = yield* metadata.Now;
									const [updated] = yield* transaction
										.update(WorkspaceGitMutationApprovals)
										.set({
											execution_started_at: updated_at,
											rejection_reason: decoded.reason,
											state: "rejected",
											updated_at,
										})
										.where(
											and(
												eq(
													WorkspaceGitMutationApprovals.approval_id,
													row.approval_id,
												),
												eq(WorkspaceGitMutationApprovals.state, "approved"),
											),
										)
										.returning();

									if (!updated || updated.decision_message_id === null) {
										return yield* invariant(
											"Approved Git mutation rejection did not persist",
										);
									}

									const approval = yield* DecodeApproval(updated);
									const event = yield* AppendEvent(
										transaction,
										approval,
										updated.decision_message_id,
										updated.approval_id,
									);

									return { approval, event, status: "accepted" as const };
								}),
							),
						).pipe(Effect.mapError(normalize_error));

						if (result.status === "accepted") {
							yield* notifier.Publish(result.event.journal_sequence);
						}

						return result;
					}),
				),
			);

		const Query = (query: typeof WorkspaceGitMutationApprovalQuery.Type) =>
			Schema.decodeUnknownEffect(WorkspaceGitMutationApprovalQuery, {
				onExcessProperty: "error",
			})(query).pipe(
				Effect.mapError(() => new WorkspaceGitMutationUnavailable({ reason: "missing" })),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, decoded.thread_id);

							const row = yield* ReadRow(transaction, decoded.approval_id);

							if (row.thread_id !== decoded.thread_id) {
								return yield* new WorkspaceGitMutationUnavailable({
									reason: "missing",
								});
							}

							const approval = yield* DecodeApproval(row);
							const artifact = yield* ReadArtifact(transaction, row.approval_id);

							if (
								!json_equals(
									approval.operation,
									summarize_workspace_git_mutation(artifact.operation),
								)
							) {
								return yield* invariant("Git mutation query binding is corrupt");
							}

							return yield* Schema.decodeUnknownEffect(
								WorkspaceGitMutationApprovalQueryResult,
								{ onExcessProperty: "error" },
							)({ approval }).pipe(
								Effect.mapError(() =>
									invariant("Git mutation query result is corrupt"),
								),
							);
						}),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadBySourceCommand = (message_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(message_id).pipe(
				Effect.mapError(() => invariant("Git mutation request command id is invalid")),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						ReadStoredRequestBinding(transaction, decoded).pipe(
							Effect.map(Option.map((binding) => binding.acceptance)),
						),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ReadActionAnchor = (input: typeof ActionAnchorQuery.Type) =>
			Schema.decodeUnknownEffect(ActionAnchorQuery, { onExcessProperty: "error" })(
				input,
			).pipe(
				Effect.mapError(
					() => new WorkspaceGitMutationConflict({ reason: "action_conflict" }),
				),
				Effect.flatMap((decoded) =>
					database.client.transaction((transaction) =>
						EnsureActionParentAvailable(transaction, decoded),
					),
				),
				Effect.mapError(normalize_error),
			);

		const ListApproved = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select({
							approval_id: WorkspaceGitMutationApprovals.approval_id,
							thread_id: WorkspaceGitMutationApprovals.thread_id,
						})
						.from(WorkspaceGitMutationApprovals)
						.where(eq(WorkspaceGitMutationApprovals.state, "approved"))
						.orderBy(
							asc(WorkspaceGitMutationApprovals.created_at),
							asc(WorkspaceGitMutationApprovals.approval_id),
						);

					yield* Effect.forEach(
						rows,
						(row) => EnsureLiveThread(transaction, row.thread_id),
						{ discard: true },
					);

					return rows;
				}),
			)
			.pipe(Effect.mapError(normalize_error));
		const ListExecuting = Effect.gen(function* () {
			const now = yield* metadata.Now;

			return yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* transaction
						.select()
						.from(WorkspaceGitMutationApprovals)
						.where(eq(WorkspaceGitMutationApprovals.state, "executing"))
						.orderBy(
							asc(WorkspaceGitMutationApprovals.created_at),
							asc(WorkspaceGitMutationApprovals.approval_id),
						);

					return yield* Effect.forEach(rows, (row) =>
						Effect.gen(function* () {
							yield* EnsureLiveThread(transaction, row.thread_id);

							const claim = yield* ReadClaim(transaction, row);
							const expired = yield* LeaseExpired(claim.lease_expires_at, now);
							const execution_safe =
								claim.execution_started_at === null ||
								claim.execution_completed_at !== null;
							const owned = claim.owner_instance_id === metadata.instance_id;
							const recovery: WorkspaceGitMutationDispatch["recovery"] =
								execution_safe
									? owned
										? "owned"
										: expired
											? "recoverable"
											: "waiting"
									: expired
										? "quarantine"
										: "waiting";

							return {
								approval_id: row.approval_id,
								recovery,
								thread_id: row.thread_id,
							};
						}),
					);
				}),
			);
		}).pipe(Effect.mapError(normalize_error));

		return {
			AbandonOwnedExecutions,
			ClaimRecovery,
			Decide,
			ExecuteClaimed,
			ListApproved,
			ListExecuting,
			MarkExecuting,
			Query,
			QuarantineInterrupted,
			ReadActionAnchor,
			ReadBySourceCommand,
			ReadExecution,
			ReplayRequest,
			RecordAttempt,
			RecordReconciliation,
			RejectApproved,
			Request,
			RenewLease,
			Settle,
		};
	}),
);
