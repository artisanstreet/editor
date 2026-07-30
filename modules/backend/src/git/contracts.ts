import { Context, Data, Effect, Schema } from "effect";

import {
	EventEnvelope,
	GitBranchState,
	GitDiffSummary,
	GitFileChange,
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
	GitMutationFailure,
	GitMutationKind,
	GitMutationLifecycle,
	GitMutationPaths,
	GitMutationResolveEnvelope,
	GitRepositoryProjection,
	GitSnapshotId,
	GitWorkspaceUpdatedEvent,
	GitWorktree,
	Identifier,
	IsoDateTime,
	JournalSequence,
	PositiveInt,
	RawOrigin,
	git_workspace_maximum_changed_paths,
	git_workspace_maximum_worktrees,
	type GitMutationProjection as GitMutationProjectionValue,
	type GitWorkspaceProjection as GitWorkspaceProjectionValue,
} from "@artisan/protocol";

export const RequestFingerprint = GitSnapshotId;
export const NullableIdentifier = Schema.Union([Identifier, Schema.Null]);
export const NullableIsoDateTime = Schema.Union([IsoDateTime, Schema.Null]);
export const NullableJournalSequence = Schema.Union([JournalSequence, Schema.Null]);
export const NullablePositiveInt = Schema.Union([PositiveInt, Schema.Null]);
export const NullableSnapshotId = Schema.Union([GitSnapshotId, Schema.Null]);
export const NullableString = Schema.Union([Schema.String, Schema.Null]);
export const git_dispatch_lease_milliseconds = 60_000;

export const GitRepositoryObservation = Schema.Struct({
	aggregate: GitDiffSummary,
	branch: GitBranchState,
	clean: Schema.Boolean,
	files: Schema.Array(GitFileChange).check(
		Schema.isMaxLength(git_workspace_maximum_changed_paths),
	),
	head: Schema.optional(GitRepositoryProjection.fields.head),
	observed_at: IsoDateTime,
	repository_state: Schema.Literal("repository"),
	snapshot_id: GitSnapshotId,
	staged: GitDiffSummary,
	unstaged: GitDiffSummary,
	workspace_id: Identifier,
	worktrees: Schema.NonEmptyArray(GitWorktree).check(
		Schema.isMaxLength(git_workspace_maximum_worktrees),
	),
});

export const GitNotRepositoryObservation = Schema.Struct({
	observed_at: IsoDateTime,
	repository_state: Schema.Literal("not_repository"),
	snapshot_id: GitSnapshotId,
	workspace_id: Identifier,
});

/** Carries one complete observed workspace without repository-owned sequence fields. */
export const GitWorkspaceObservation = Schema.Union([
	GitNotRepositoryObservation,
	GitRepositoryObservation,
]);

export type GitWorkspaceObservation = typeof GitWorkspaceObservation.Type;

export const GitEventTrace = {
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
};

export const GitWorkspaceRecordInput = Schema.Struct({
	...GitEventTrace,
	cause: GitWorkspaceUpdatedEvent.fields.cause,
	workspace: GitWorkspaceObservation,
});

export type GitWorkspaceRecordInput = typeof GitWorkspaceRecordInput.Type;

export const GitMutationSucceededInput = Schema.Struct({
	mutation_id: Identifier,
	workspace: GitWorkspaceObservation,
});

export type GitMutationSucceededInput = typeof GitMutationSucceededInput.Type;

export const GitMutationTerminalInput = Schema.Struct({
	failure: GitMutationFailure,
	mutation_id: Identifier,
	state: Schema.Literals(["failed", "ambiguous"]),
});

export type GitMutationTerminalInput = typeof GitMutationTerminalInput.Type;

export const GitMutationRequestEnvelope = Schema.Union([
	GitIndexStageRequestEnvelope,
	GitIndexUnstageRequestEnvelope,
]);

export type GitMutationRequestEnvelope = typeof GitMutationRequestEnvelope.Type;

export const StoredGitWorkspaceRow = Schema.Struct({
	journal_sequence: JournalSequence,
	observed_at: IsoDateTime,
	projection_json: Schema.String,
	snapshot_id: GitSnapshotId,
	updated_at: IsoDateTime,
	version: PositiveInt,
	workspace_id: Identifier,
});

export const StoredGitMutationRow = Schema.Struct({
	agent_id: NullableIdentifier,
	approval_id: Identifier,
	completed_at: NullableIsoDateTime,
	decision_at: NullableIsoDateTime,
	decision_message_id: NullableIdentifier,
	dispatched_at: NullableIsoDateTime,
	dispatch_lease_expires_at: NullableIsoDateTime,
	dispatch_owner_id: NullableIdentifier,
	expected_snapshot_id: GitSnapshotId,
	expected_workspace_version: PositiveInt,
	failure_code: NullableString,
	journal_sequence: NullableJournalSequence,
	kind: GitMutationKind,
	lifecycle: GitMutationLifecycle,
	mutation_id: Identifier,
	paths_json: Schema.String,
	raw_origin_json: NullableString,
	request_fingerprint: RequestFingerprint,
	requested_at: IsoDateTime,
	result_snapshot_id: NullableSnapshotId,
	result_workspace_version: NullablePositiveInt,
	run_id: NullableIdentifier,
	source_message_id: Identifier,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
});

export const StoredJournalEventRow = Schema.Struct({
	agent_id: NullableIdentifier,
	causation_id: Identifier,
	correlation_id: Identifier,
	event_id: Identifier,
	event_type: Identifier,
	journal_sequence: JournalSequence,
	occurred_at: IsoDateTime,
	origin: Schema.Literal("backend"),
	payload_json: Schema.String,
	protocol_version: Schema.optional(Schema.Literal(1)),
	raw_origin_json: NullableString,
	run_id: NullableIdentifier,
	schema_version: Schema.Literal(1),
	stream_id: Identifier,
	stream_sequence: JournalSequence,
	thread_id: Identifier,
});

export interface MutationIdentity {
	readonly agent_id?: string;
	readonly approval_id: string;
	readonly expected_snapshot_id: string;
	readonly expected_workspace_version: number;
	readonly kind: typeof GitMutationKind.Type;
	readonly mutation_id: string;
	readonly paths: typeof GitMutationPaths.Type;
	readonly raw_origin?: typeof RawOrigin.Type;
	readonly request_message_id: string;
	readonly requested_at: string;
	readonly run_id?: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

export interface DecodedMutationRow {
	readonly identity: MutationIdentity;
	readonly projection: GitMutationProjectionValue;
	readonly request_fingerprint: string;
	readonly result_snapshot_id?: string;
	readonly row: typeof StoredGitMutationRow.Type;
}

export interface GitMutationAcceptance {
	readonly event: typeof EventEnvelope.Type;
	readonly mutation: GitMutationProjectionValue;
	readonly status: "accepted" | "duplicate";
}

export interface GitWorkspaceCommit {
	readonly event: typeof EventEnvelope.Type;
	readonly status: "accepted" | "duplicate";
	readonly workspace: GitWorkspaceProjectionValue;
}

export interface GitMutationSuccessCommit {
	readonly mutation: GitMutationProjectionValue;
	readonly mutation_event: typeof EventEnvelope.Type;
	readonly status: "accepted" | "duplicate";
	readonly workspace: GitWorkspaceProjectionValue;
	readonly workspace_event: typeof EventEnvelope.Type;
}

export interface GitRepositoryRecovery {
	readonly ambiguous: ReadonlyArray<GitMutationProjectionValue>;
	readonly approved: ReadonlyArray<GitMutationProjectionValue>;
}

export type GitRepositoryConflictReason =
	| "decision_conflict"
	| "dispatch_conflict"
	| "mutation_conflict"
	| "terminal_conflict"
	| "thread_unavailable"
	| "workspace_busy"
	| "workspace_changed";

/** Reports malformed repository input before persistence is touched. */
export class GitRepositoryInvalid extends Data.TaggedError("GitRepositoryInvalid")<{
	readonly operation: string;
}> {}

/** Reports exact identity reuse or optimistic concurrency failure without side effects. */
export class GitRepositoryConflict extends Data.TaggedError("GitRepositoryConflict")<{
	readonly reason: GitRepositoryConflictReason;
}> {}

/** Reports a missing durable Git workspace or mutation without leaking another identity. */
export class GitRepositoryNotFound extends Data.TaggedError("GitRepositoryNotFound")<{
	readonly resource: "mutation" | "workspace";
}> {}

/** Reports malformed or internally inconsistent durable Git state. */
export class GitRepositoryInvariantError extends Data.TaggedError("GitRepositoryInvariantError")<{
	readonly message: string;
}> {}

/** Conceals unexpected SQLite and cryptographic infrastructure failures. */
export class GitRepositoryPersistenceFailure extends Data.TaggedError(
	"GitRepositoryPersistenceFailure",
)<{ readonly cause: unknown }> {}

export type GitRepositoryError =
	| GitRepositoryConflict
	| GitRepositoryInvalid
	| GitRepositoryInvariantError
	| GitRepositoryNotFound
	| GitRepositoryPersistenceFailure;

/** Owns durable Git workspace observations and approval-bound mutation lifecycles. */
export class GitRepository extends Context.Service<
	GitRepository,
	{
		readonly ClaimApproved: (
			mutation_id: string,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly CommitSucceeded: (
			input: GitMutationSucceededInput,
		) => Effect.Effect<GitMutationSuccessCommit, GitRepositoryError>;
		readonly CommitTerminal: (
			input: GitMutationTerminalInput,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly ListPending: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<GitMutationProjectionValue>, GitRepositoryError>;
		readonly ReadMutation: (
			mutation_id: string,
		) => Effect.Effect<GitMutationProjectionValue, GitRepositoryError>;
		readonly ReadWorkspace: (
			workspace_id: string,
		) => Effect.Effect<GitWorkspaceProjectionValue, GitRepositoryError>;
		readonly RecordWorkspace: (
			input: GitWorkspaceRecordInput,
		) => Effect.Effect<GitWorkspaceCommit, GitRepositoryError>;
		readonly RecoverDispatching: () => Effect.Effect<GitRepositoryRecovery, GitRepositoryError>;
		readonly RequestMutation: (
			envelope: GitMutationRequestEnvelope,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
		readonly ResolveMutation: (
			envelope: typeof GitMutationResolveEnvelope.Type,
		) => Effect.Effect<GitMutationAcceptance, GitRepositoryError>;
	}
>()("Artisan/GitRepository") {}
