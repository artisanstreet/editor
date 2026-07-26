import { Context, Data, Effect, Option, Schema } from "effect";

import {
	ContentIdentity,
	Identifier,
	IsoDateTime,
	RawOrigin,
	WorkspaceChangeUpdatedEvent,
	WorkspacePath,
	WorkspaceReviewerKind,
	WorkspaceReviewOutcome,
	WorkspaceReviewText,
	workspace_diff_format_version,
	type ContentIdentity as ContentIdentityValue,
	type RawOrigin as RawOriginValue,
	type WorkspaceChange as WorkspaceChangeValue,
	type WorkspaceConflict as WorkspaceConflictValue,
} from "@artisan/protocol";

import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
} from "../../persistence/journal-store";
import type { PreparedWorkspaceChangeDiff } from "../workspace-change-diff-service";

const WorkspaceChangeLifecycle = Schema.Literals(["claimed", "applied", "committed", "rejected"]);
const RequestFingerprint = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/));
const JournalSequence = Schema.Int.check(Schema.isGreaterThanOrEqualTo(1));

const WorkspaceChangeOperationBase = {
	change_id: Identifier,
	diff_format_version: Schema.Literal(workspace_diff_format_version),
	evidence_recorded: Schema.Boolean,
	journal_sequence: Schema.optional(JournalSequence),
	lifecycle: WorkspaceChangeLifecycle,
	message_id: Identifier,
	request_fingerprint: RequestFingerprint,
	sent_at: IsoDateTime,
	thread_id: Identifier,
};

export const WorkspaceChangeOperationSchema = Schema.Union([
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("replace"),
		agent_id: Identifier,
		expected_identity: ContentIdentity,
		path: WorkspacePath,
		raw_origin: Schema.optional(RawOrigin),
		result_identity: ContentIdentity,
		run_id: Identifier,
		workspace_id: Identifier,
	}),
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("review"),
		reviewer_kind: WorkspaceReviewerKind,
		assignment_id: Schema.optional(Identifier),
		comment: Schema.optional(WorkspaceReviewText),
		group_id: Schema.optional(Identifier),
		outcome: Schema.optional(WorkspaceReviewOutcome),
		raw_origin: Schema.optional(RawOrigin),
		reviewer_agent_id: Schema.optional(Identifier),
		reviewer_run_id: Schema.optional(Identifier),
	}),
	Schema.Struct({
		...WorkspaceChangeOperationBase,
		action: Schema.Literal("rollback"),
		expected_identity: ContentIdentity,
	}),
]);

export const WorkspaceChangeCommandIdentity = Schema.Struct({
	action: Schema.Literals(["replace", "review", "rollback"]),
	change_id: Identifier,
	request_fingerprint: RequestFingerprint,
	type: Schema.Literal("workspace.change.command"),
});

export const WorkspaceChangeJournalEvent = Schema.Struct({
	causation_id: Identifier,
	correlation_id: Identifier,
	event_id: Identifier,
	journal_sequence: JournalSequence,
	occurred_at: IsoDateTime,
	payload: WorkspaceChangeUpdatedEvent,
	sequence: JournalSequence,
});

/** Identifies a replacement operation before filesystem mutation. */
export interface ClaimReplace {
	readonly _tag: "replace";
	readonly agent_id: string;
	readonly change_id: string;
	readonly expected_before: ContentIdentityValue;
	readonly intended_after: ContentIdentityValue;
	readonly message_id: string;
	readonly path: string;
	readonly raw_origin?: RawOriginValue;
	readonly request_fingerprint: string;
	readonly run_id: string;
	readonly sent_at: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Identifies a user review operation before projection transition. */
export interface ClaimReview {
	readonly _tag: "review";
	readonly change_id: string;
	readonly message_id: string;
	readonly request_fingerprint: string;
	readonly sent_at: string;
	readonly thread_id: string;
	readonly reviewer_kind?: "user" | "graph";
	readonly assignment_id?: string;
	readonly comment?: string;
	readonly group_id?: string;
	readonly outcome?: "approved" | "changes_requested";
	readonly raw_origin?: RawOriginValue;
	readonly reviewer_agent_id?: string;
	readonly reviewer_run_id?: string;
}

/** Identifies a guarded rollback operation before filesystem mutation. */
export interface ClaimRollback {
	readonly _tag: "rollback";
	readonly change_id: string;
	readonly expected_after: ContentIdentityValue;
	readonly message_id: string;
	readonly request_fingerprint: string;
	readonly sent_at: string;
	readonly thread_id: string;
}

/** Represents the immutable identity of a workspace operation. */
export type WorkspaceChangeOperation = typeof WorkspaceChangeOperationSchema.Type;

/** Returns the result of claiming a workspace operation. */
export type WorkspaceChangeClaim =
	| { readonly _tag: "claimed"; readonly operation: WorkspaceChangeOperation }
	| { readonly _tag: "incomplete_retry"; readonly operation: WorkspaceChangeOperation }
	| { readonly _tag: "rejected"; readonly operation: WorkspaceChangeOperation }
	| {
			readonly _tag: "duplicate";
			readonly event: WorkspaceChangeEvent;
			readonly operation: WorkspaceChangeOperation;
	  };

/** Carries one stored workspace-change journal event. */
export type WorkspaceChangeEvent = typeof WorkspaceChangeJournalEvent.Type;

/** Returns an accepted transition or its exact duplicate. */
export interface WorkspaceChangeCommit {
	readonly event: WorkspaceChangeEvent;
	readonly status: "accepted" | "duplicate";
}

/** Resolves a native changed observation against one exact durable operation. */
export type WorkspaceChangeReconciliation =
	| { readonly _tag: "applied"; readonly operation: WorkspaceChangeOperation }
	| {
			readonly _tag: "committed";
			readonly event: WorkspaceChangeEvent;
			readonly operation: WorkspaceChangeOperation;
	  }
	| { readonly _tag: "rejected"; readonly operation: WorkspaceChangeOperation }
	| { readonly _tag: "staged"; readonly operation: WorkspaceChangeOperation };

/** Identifies where a changed file observation occurred in mutation execution. */
export interface ReconcileWorkspaceChange {
	readonly message_id: string;
	readonly observation: "native_changed" | "preflight_changed";
	readonly observed_identity?: ContentIdentityValue;
}

/** Reports an immutable collision between distinct replacement operations. */
export class WorkspaceChangeIdConflict extends Data.TaggedError("WorkspaceChangeIdConflict")<{
	readonly change_id: string;
}> {}

/** Reports an invalid operation lifecycle, action, or change transition. */
export class WorkspaceChangeTransitionError extends Data.TaggedError(
	"WorkspaceChangeTransitionError",
)<{ readonly message: string }> {}

/** Represents failures surfaced by the workspace change repository. */
export type WorkspaceChangeRepositoryError =
	| CommandIdConflict
	| JournalInvariantError
	| JournalStoreFailure
	| WorkspaceChangeIdConflict
	| WorkspaceChangeTransitionError;

/** Owns durable, source-free workspace change operations and projections. */
export class WorkspaceChangeRepository extends Context.Service<
	WorkspaceChangeRepository,
	{
		readonly ClaimReplace: (
			input: ClaimReplace,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly ClaimReview: (
			input: ClaimReview,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly ClaimRollback: (
			input: ClaimRollback,
		) => Effect.Effect<WorkspaceChangeClaim, WorkspaceChangeRepositoryError>;
		readonly MarkApplied: (
			input:
				| {
						readonly _tag: "replace";
						readonly message_id: string;
						readonly result_identity: ContentIdentityValue;
				  }
				| { readonly _tag: "rollback"; readonly message_id: string },
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly RejectChanged: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly ReconcileChanged: (
			input: ReconcileWorkspaceChange,
		) => Effect.Effect<WorkspaceChangeReconciliation, WorkspaceChangeRepositoryError>;
		readonly CommitRecorded: (
			message_id: string,
			prepared_diff: PreparedWorkspaceChangeDiff,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly CommitReviewed: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly CommitRolledBack: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeCommit, WorkspaceChangeRepositoryError>;
		readonly MarkEvidenceRecorded: (
			message_id: string,
		) => Effect.Effect<WorkspaceChangeOperation, WorkspaceChangeRepositoryError>;
		readonly ReadChange: (
			change_id: string,
		) => Effect.Effect<Option.Option<WorkspaceChangeValue>, WorkspaceChangeRepositoryError>;
		readonly ReadOperation: (
			message_id: string,
		) => Effect.Effect<Option.Option<WorkspaceChangeOperation>, WorkspaceChangeRepositoryError>;
		readonly List: (
			thread_id: string,
			workspace_id?: string,
		) => Effect.Effect<
			{
				readonly changes: ReadonlyArray<WorkspaceChangeValue>;
				readonly journal_sequence: number;
			},
			WorkspaceChangeRepositoryError
		>;
		readonly ListConflicts: (
			thread_id: string,
		) => Effect.Effect<ReadonlyArray<WorkspaceConflictValue>, WorkspaceChangeRepositoryError>;
		readonly ListConflictSnapshot: (thread_id: string) => Effect.Effect<
			{
				readonly conflicts: ReadonlyArray<WorkspaceConflictValue>;
				readonly journal_sequence: number;
			},
			WorkspaceChangeRepositoryError
		>;
	}
>()("Artisan/WorkspaceChangeRepository") {}
