import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt } from "./common";
import { WorkspacePath } from "./workspace-changes";

const text_encoder = new TextEncoder();

/** Validates a bounded visible Git branch name without control characters. */
export const GitBranchName = Schema.String.check(
	Schema.makeFilter<string>((branch) => {
		const byte_count = text_encoder.encode(branch).byteLength;

		return branch.trim().length === 0 || byte_count > 255 || /[\p{Cc}]/u.test(branch)
			? "Expected a bounded visible Git branch name without control characters"
			: undefined;
	}),
);

export type GitBranchName = typeof GitBranchName.Type;

/** Validates a lowercase SHA-1 or SHA-256 Git object identifier. */
export const GitObjectId = Schema.String.check(
	Schema.isPattern(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/, {
		message: "Expected a lowercase 40- or 64-character Git object identifier",
	}),
);

export type GitObjectId = typeof GitObjectId.Type;

/** Identifies the selected or an external Git worktree without exposing its path. */
export const WorkspaceGitWorktree = Schema.Struct({
	bare: Schema.Boolean,
	branch: Schema.optional(GitBranchName),
	detached: Schema.Boolean,
	head: Schema.optional(GitObjectId),
	locked: Schema.Boolean,
	location: Schema.Literals(["selected", "external"]),
	prunable: Schema.Boolean,
});

export type WorkspaceGitWorktree = typeof WorkspaceGitWorktree.Type;

/** Summarizes one changed canonical workspace path reported by Git. */
export const WorkspaceGitChangedFile = Schema.Struct({
	conflicted: Schema.Boolean,
	original_path: Schema.optional(WorkspacePath),
	path: WorkspacePath,
	staged: Schema.Boolean,
	status: Schema.String.check(
		Schema.makeFilter<string>((status) =>
			status.length === 0 || status.length > 64 || /[\p{Cc}]/u.test(status)
				? "Expected a bounded visible Git status without control characters"
				: undefined,
		),
	),
	untracked: Schema.Boolean,
	unstaged: Schema.Boolean,
});

export type WorkspaceGitChangedFile = typeof WorkspaceGitChangedFile.Type;

/** Reports aggregate Git diff size without source content. */
export const GitDiffStats = Schema.Struct({
	additions: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	deletions: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	files: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});

export type GitDiffStats = typeof GitDiffStats.Type;

/** Explains why a Git session cannot safely accept a checkout request. */
export const WorkspaceGitSessionBlocker = Schema.Literals([
	"not_repository",
	"selected_worktree_missing",
	"multiple_worktrees",
	"bare_repository",
	"detached_head",
	"locked_worktree",
	"prunable_worktree",
	"selected_worktree_mismatch",
	"unborn_head",
]);

export type WorkspaceGitSessionBlocker = typeof WorkspaceGitSessionBlocker.Type;

/** Represents the availability of the selected workspace's Git state. */
export const WorkspaceGitSessionState = Schema.Literals(["ready", "blocked", "unavailable"]);

export type WorkspaceGitSessionState = typeof WorkspaceGitSessionState.Type;

/** Projects the bounded, provider-neutral Git state of one workspace. */
export const WorkspaceGitSession = Schema.Struct({
	blockers: Schema.Array(WorkspaceGitSessionBlocker),
	branch: Schema.optional(GitBranchName),
	changed_files: Schema.Array(WorkspaceGitChangedFile),
	diff_stats: GitDiffStats,
	has_diff: Schema.Boolean,
	head: Schema.optional(GitObjectId),
	journal_sequence: JournalSequence,
	observed_at: IsoDateTime,
	state: WorkspaceGitSessionState,
	version: PositiveInt,
	worktrees: Schema.Array(WorkspaceGitWorktree),
	workspace_id: Identifier,
});

export type WorkspaceGitSession = typeof WorkspaceGitSession.Type;

/** Requests the current Git session projection for one workspace. */
export const WorkspaceGitSessionQuery = Schema.Struct({ workspace_id: Identifier });

export type WorkspaceGitSessionQuery = typeof WorkspaceGitSessionQuery.Type;

/** Returns an optional Git session projection at a durable journal position. */
export const WorkspaceGitSessionQueryResult = Schema.Struct({
	journal_sequence: JournalSequence,
	session: Schema.optional(WorkspaceGitSession),
});

export type WorkspaceGitSessionQueryResult = typeof WorkspaceGitSessionQueryResult.Type;

/** Requests a fresh observation of one workspace's Git session. */
export const WorkspaceGitSessionRefreshRequest = Schema.Struct({ workspace_id: Identifier });

export type WorkspaceGitSessionRefreshRequest = typeof WorkspaceGitSessionRefreshRequest.Type;

/** Requests a guarded checkout of one target branch. */
export const WorkspaceGitCheckoutRequest = Schema.Struct({
	expected_session_version: PositiveInt,
	target_branch: GitBranchName,
	workspace_id: Identifier,
});

export type WorkspaceGitCheckoutRequest = typeof WorkspaceGitCheckoutRequest.Type;

const WorkspaceGitCheckoutApprovalBase = {
	approval_id: Identifier,
	created_at: IsoDateTime,
	expected_session_version: PositiveInt,
	source_branch: Schema.optional(GitBranchName),
	source_command_id: Identifier,
	source_head: Schema.optional(GitObjectId),
	target_branch: GitBranchName,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
};

const WorkspaceGitCheckoutApprovalDecision = {
	decided_at: IsoDateTime,
	decision_message_id: Identifier,
};

/** Projects a source-free checkout approval awaiting a user decision. */
export const WorkspaceGitCheckoutApprovalRequested = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	state: Schema.Literal("requested"),
});

/** Projects an approved checkout before it starts execution. */
export const WorkspaceGitCheckoutApprovalApproved = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("approved"),
});

/** Projects an approved checkout while it is executing. */
export const WorkspaceGitCheckoutApprovalExecuting = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("executing"),
});

/** Projects an approved checkout that was applied. */
export const WorkspaceGitCheckoutApprovalApplied = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("applied"),
});

/** Projects an approved checkout that could not be completed. */
export const WorkspaceGitCheckoutApprovalRejected = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("rejected"),
});

/** Projects an approved checkout whose terminal outcome cannot be established. */
export const WorkspaceGitCheckoutApprovalUnknown = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("approved"),
	state: Schema.Literal("unknown"),
});

/** Projects a checkout explicitly denied by a user. */
export const WorkspaceGitCheckoutApprovalDenied = Schema.Struct({
	...WorkspaceGitCheckoutApprovalBase,
	...WorkspaceGitCheckoutApprovalDecision,
	decision: Schema.Literal("denied"),
	state: Schema.Literal("denied"),
});

/** Represents every source-free lifecycle state for one checkout approval. */
export const WorkspaceGitCheckoutApproval = Schema.Union([
	WorkspaceGitCheckoutApprovalRequested,
	WorkspaceGitCheckoutApprovalApproved,
	WorkspaceGitCheckoutApprovalExecuting,
	WorkspaceGitCheckoutApprovalApplied,
	WorkspaceGitCheckoutApprovalRejected,
	WorkspaceGitCheckoutApprovalUnknown,
	WorkspaceGitCheckoutApprovalDenied,
]);

export type WorkspaceGitCheckoutApproval = typeof WorkspaceGitCheckoutApproval.Type;

/** Requests one checkout approval by its durable identity. */
export const WorkspaceGitCheckoutApprovalQuery = Schema.Struct({
	approval_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceGitCheckoutApprovalQuery = typeof WorkspaceGitCheckoutApprovalQuery.Type;

/** Returns one source-free checkout approval projection. */
export const WorkspaceGitCheckoutApprovalQueryResult = Schema.Struct({
	approval: WorkspaceGitCheckoutApproval,
});

export type WorkspaceGitCheckoutApprovalQueryResult =
	typeof WorkspaceGitCheckoutApprovalQueryResult.Type;

/** Records a user decision for one pending checkout approval. */
export const WorkspaceGitCheckoutApprovalResponseRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
});

export type WorkspaceGitCheckoutApprovalResponseRequest =
	typeof WorkspaceGitCheckoutApprovalResponseRequest.Type;

/** Announces an updated workspace Git session projection. */
export const WorkspaceGitSessionUpdatedEvent = Schema.Struct({
	session: WorkspaceGitSession,
	type: Schema.Literal("workspace.git.session.updated"),
});

export type WorkspaceGitSessionUpdatedEvent = typeof WorkspaceGitSessionUpdatedEvent.Type;

/** Announces one source-free checkout approval lifecycle update. */
export const WorkspaceGitCheckoutApprovalUpdatedEvent = Schema.Struct({
	approval: WorkspaceGitCheckoutApproval,
	type: Schema.Literal("workspace.git.checkout.approval.updated"),
});

export type WorkspaceGitCheckoutApprovalUpdatedEvent =
	typeof WorkspaceGitCheckoutApprovalUpdatedEvent.Type;
