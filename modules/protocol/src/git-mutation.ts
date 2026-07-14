import { Schema } from "effect";

import { Identifier, IsoDateTime, PositiveInt } from "./common";
import { GitBranchName, GitObjectId } from "./git-session";

const text_encoder = new TextEncoder();

/** Validates a conservative local branch name that is safe as a Git argument. */
export const GitLocalBranchName = GitBranchName.check(
	Schema.makeFilter<string>((branch) => {
		const has_invalid_component = branch
			.split("/")
			.some(
				(component) =>
					component === "." || component === ".." || component.endsWith(".lock"),
			);
		const has_invalid_character = /[\s~^:?*[\\]/u.test(branch);

		return branch.startsWith("-") ||
			branch === "@" ||
			branch.startsWith(".") ||
			branch.endsWith(".") ||
			branch.startsWith("/") ||
			branch.endsWith("/") ||
			branch.includes("//") ||
			branch.includes("..") ||
			branch.includes("@{") ||
			has_invalid_component ||
			has_invalid_character
			? "Expected a safe local Git branch name"
			: undefined;
	}),
);

export type GitLocalBranchName = typeof GitLocalBranchName.Type;

/** Validates a conservative local Git remote name without option-like syntax. */
export const GitRemoteName = Schema.String.check(
	Schema.isPattern(/^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/, {
		message: "Expected a conservative Git remote name",
	}),
);

export type GitRemoteName = typeof GitRemoteName.Type;

/** Validates a bounded non-empty commit message without NUL bytes. */
export const GitCommitMessage = Schema.String.check(
	Schema.makeFilter<string>((message) => {
		const byte_count = text_encoder.encode(message).byteLength;

		return message.trim().length === 0 || byte_count > 4096 || message.includes("\0")
			? "Expected a non-empty Git commit message of at most 4096 UTF-8 bytes without NUL"
			: undefined;
	}),
);

export type GitCommitMessage = typeof GitCommitMessage.Type;

/** Enumerates the explicit whole-repository reset modes supported by Artisan. */
export const GitResetMode = Schema.Literals(["soft", "mixed", "hard"]);

export type GitResetMode = typeof GitResetMode.Type;

const WorkspaceGitMergeStartOperation = Schema.Struct({
	action: Schema.Literal("start"),
	target_branch: GitLocalBranchName,
	type: Schema.Literal("merge"),
});

const WorkspaceGitMergeContinuationOperation = Schema.Union([
	Schema.Struct({ action: Schema.Literal("continue"), type: Schema.Literal("merge") }),
	Schema.Struct({ action: Schema.Literal("abort"), type: Schema.Literal("merge") }),
]);

const WorkspaceGitRebaseStartOperation = Schema.Struct({
	action: Schema.Literal("start"),
	target_branch: GitLocalBranchName,
	type: Schema.Literal("rebase"),
});

const WorkspaceGitRebaseContinuationOperation = Schema.Union([
	Schema.Struct({ action: Schema.Literal("continue"), type: Schema.Literal("rebase") }),
	Schema.Struct({ action: Schema.Literal("abort"), type: Schema.Literal("rebase") }),
	Schema.Struct({ action: Schema.Literal("skip"), type: Schema.Literal("rebase") }),
]);

/** Describes a Git conflict action that must name its exact prior approval. */
export const WorkspaceGitMutationContinuationOperation = Schema.Union([
	WorkspaceGitMergeContinuationOperation,
	WorkspaceGitRebaseContinuationOperation,
]);

export type WorkspaceGitMutationContinuationOperation =
	typeof WorkspaceGitMutationContinuationOperation.Type;

/** Describes a Git mutation that does not consume a prior conflict approval. */
export const WorkspaceGitMutationStandaloneOperation = Schema.Union([
	Schema.Struct({ branch: GitLocalBranchName, type: Schema.Literal("branch_create") }),
	Schema.Struct({ target_branch: GitLocalBranchName, type: Schema.Literal("checkout") }),
	Schema.Struct({ mode: GitResetMode, target: GitObjectId, type: Schema.Literal("reset") }),
	Schema.Struct({ type: Schema.Literal("clean") }),
	Schema.Struct({ message: GitCommitMessage, type: Schema.Literal("commit") }),
	WorkspaceGitMergeStartOperation,
	WorkspaceGitRebaseStartOperation,
	Schema.Struct({ type: Schema.Literal("pull_ff_only") }),
	Schema.Struct({
		remote: GitRemoteName,
		set_upstream: Schema.Boolean,
		target_branch: GitLocalBranchName,
		type: Schema.Literal("push"),
	}),
]);

export type WorkspaceGitMutationStandaloneOperation =
	typeof WorkspaceGitMutationStandaloneOperation.Type;

const WorkspaceGitMergeOperation = Schema.Union([
	Schema.Struct({
		action: Schema.Literal("start"),
		target_branch: GitLocalBranchName,
		type: Schema.Literal("merge"),
	}),
	WorkspaceGitMergeContinuationOperation,
]);

const WorkspaceGitRebaseOperation = Schema.Union([
	Schema.Struct({
		action: Schema.Literal("start"),
		target_branch: GitLocalBranchName,
		type: Schema.Literal("rebase"),
	}),
	WorkspaceGitRebaseContinuationOperation,
]);

/** Describes the complete local Git intent that requires an explicit approval. */
export const WorkspaceGitMutationOperation = Schema.Union([
	Schema.Struct({ branch: GitLocalBranchName, type: Schema.Literal("branch_create") }),
	Schema.Struct({ target_branch: GitLocalBranchName, type: Schema.Literal("checkout") }),
	Schema.Struct({ mode: GitResetMode, target: GitObjectId, type: Schema.Literal("reset") }),
	Schema.Struct({ type: Schema.Literal("clean") }),
	Schema.Struct({ message: GitCommitMessage, type: Schema.Literal("commit") }),
	WorkspaceGitMergeOperation,
	WorkspaceGitRebaseOperation,
	Schema.Struct({ type: Schema.Literal("pull_ff_only") }),
	Schema.Struct({
		remote: GitRemoteName,
		set_upstream: Schema.Boolean,
		target_branch: GitLocalBranchName,
		type: Schema.Literal("push"),
	}),
]);

export type WorkspaceGitMutationOperation = typeof WorkspaceGitMutationOperation.Type;

/** Summarizes an approved Git operation without carrying commit text or process output. */
export const WorkspaceGitMutationSummary = Schema.Union([
	Schema.Struct({ branch: GitLocalBranchName, type: Schema.Literal("branch_create") }),
	Schema.Struct({ target_branch: GitLocalBranchName, type: Schema.Literal("checkout") }),
	Schema.Struct({ mode: GitResetMode, target: GitObjectId, type: Schema.Literal("reset") }),
	Schema.Struct({ type: Schema.Literal("clean") }),
	Schema.Struct({ type: Schema.Literal("commit") }),
	WorkspaceGitMergeOperation,
	WorkspaceGitRebaseOperation,
	Schema.Struct({ type: Schema.Literal("pull_ff_only") }),
	Schema.Struct({
		remote: GitRemoteName,
		set_upstream: Schema.Boolean,
		target_branch: GitLocalBranchName,
		type: Schema.Literal("push"),
	}),
]);

export type WorkspaceGitMutationSummary = typeof WorkspaceGitMutationSummary.Type;

/** Removes private operation fields before an intent enters a public projection or event. */
export function summarize_workspace_git_mutation(
	operation: WorkspaceGitMutationOperation,
): WorkspaceGitMutationSummary {
	if (operation.type === "commit") {
		return { type: "commit" };
	}

	return operation;
}

/** Requests approval for one guarded local Git mutation against an observed session version. */
const WorkspaceGitMutationRequestBase = {
	expected_session_version: PositiveInt,
	workspace_id: Identifier,
};

/** Binds standalone intent or an exact prior conflict approval to one guarded request. */
export const WorkspaceGitMutationRequest = Schema.Union([
	Schema.Struct({
		...WorkspaceGitMutationRequestBase,
		operation: WorkspaceGitMutationStandaloneOperation,
	}),
	Schema.Struct({
		...WorkspaceGitMutationRequestBase,
		action_approval_id: Identifier,
		operation: WorkspaceGitMutationContinuationOperation,
	}),
]);

export type WorkspaceGitMutationRequest = typeof WorkspaceGitMutationRequest.Type;

const WorkspaceGitMutationApprovalBase = {
	action_approval_id: Schema.optional(Identifier),
	approval_id: Identifier,
	created_at: IsoDateTime,
	expected_session_version: PositiveInt,
	operation: WorkspaceGitMutationSummary,
	/** Branch and head mirror public session facts; private proof identities remain backend-only. */
	source_branch: Schema.optional(GitBranchName),
	source_command_id: Identifier,
	source_head: GitObjectId,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
};

const WorkspaceGitMutationApprovalDecision = {
	decided_at: IsoDateTime,
	decision: Schema.Literal("approved"),
	decision_message_id: Identifier,
};

/** Enumerates stable public reasons why an approved Git mutation was not applied. */
export const WorkspaceGitMutationRejectionReason = Schema.Literals([
	"branch_exists",
	"branch_missing",
	"conflicts_unresolved",
	"git_rejected",
	"invalid_state",
	"non_fast_forward",
	"nothing_to_do",
	"remote_changed",
	"remote_rejected",
	"stale_session",
	"upstream_missing",
	"workspace_changed",
]);

export type WorkspaceGitMutationRejectionReason = typeof WorkspaceGitMutationRejectionReason.Type;

/** Enumerates stable public reasons why execution cannot be reconciled safely. */
export const WorkspaceGitMutationUnknownReason = Schema.Literals([
	"interrupted",
	"remote_unverifiable",
	"state_unavailable",
	"verification_failed",
]);

export type WorkspaceGitMutationUnknownReason = typeof WorkspaceGitMutationUnknownReason.Type;

/** Projects a Git mutation awaiting a user decision without exposing commit text. */
export const WorkspaceGitMutationApprovalRequested = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	state: Schema.Literal("requested"),
});

/** Projects a Git mutation approved before it starts execution. */
export const WorkspaceGitMutationApprovalApproved = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	state: Schema.Literal("approved"),
});

/** Projects a Git mutation currently executing. */
export const WorkspaceGitMutationApprovalExecuting = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	state: Schema.Literal("executing"),
});

/** Projects an applied Git mutation with its resulting repository facts. */
export const WorkspaceGitMutationApprovalApplied = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	resulting_branch: Schema.optional(GitBranchName),
	resulting_head: GitObjectId,
	remote_head: Schema.optional(GitObjectId),
	state: Schema.Literal("applied"),
});

/** Projects a Git mutation that requires a user to resolve a merge or rebase conflict. */
export const WorkspaceGitMutationApprovalActionRequired = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	action: Schema.Literals(["merge_conflict", "rebase_conflict"]),
	state: Schema.Literal("action_required"),
});

/** Projects an approved Git mutation rejected for one stable public reason. */
export const WorkspaceGitMutationApprovalRejected = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	reason: WorkspaceGitMutationRejectionReason,
	state: Schema.Literal("rejected"),
});

/** Projects an approved Git mutation whose terminal state cannot be verified safely. */
export const WorkspaceGitMutationApprovalOutcomeUnknown = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	...WorkspaceGitMutationApprovalDecision,
	reason: WorkspaceGitMutationUnknownReason,
	state: Schema.Literal("outcome_unknown"),
});

/** Projects a Git mutation explicitly denied by a user. */
export const WorkspaceGitMutationApprovalDenied = Schema.Struct({
	...WorkspaceGitMutationApprovalBase,
	decided_at: IsoDateTime,
	decision: Schema.Literal("denied"),
	decision_message_id: Identifier,
	state: Schema.Literal("denied"),
});

/** Represents public lifecycle state without private plans, proofs, anchors, or output. */
export const WorkspaceGitMutationApproval = Schema.Union([
	WorkspaceGitMutationApprovalRequested,
	WorkspaceGitMutationApprovalApproved,
	WorkspaceGitMutationApprovalExecuting,
	WorkspaceGitMutationApprovalApplied,
	WorkspaceGitMutationApprovalActionRequired,
	WorkspaceGitMutationApprovalRejected,
	WorkspaceGitMutationApprovalOutcomeUnknown,
	WorkspaceGitMutationApprovalDenied,
]);

export type WorkspaceGitMutationApproval = typeof WorkspaceGitMutationApproval.Type;

/** Requests a public Git mutation approval projection by durable identity. */
export const WorkspaceGitMutationApprovalQuery = Schema.Struct({
	approval_id: Identifier,
	thread_id: Identifier,
});

export type WorkspaceGitMutationApprovalQuery = typeof WorkspaceGitMutationApprovalQuery.Type;

/** Returns one public Git mutation approval projection. */
export const WorkspaceGitMutationApprovalQueryResult = Schema.Struct({
	approval: WorkspaceGitMutationApproval,
});

export type WorkspaceGitMutationApprovalQueryResult =
	typeof WorkspaceGitMutationApprovalQueryResult.Type;

/** Records the user's decision for one pending Git mutation approval. */
export const WorkspaceGitMutationApprovalResponseRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
});

export type WorkspaceGitMutationApprovalResponseRequest =
	typeof WorkspaceGitMutationApprovalResponseRequest.Type;

/** Announces one public Git mutation approval lifecycle update. */
export const WorkspaceGitMutationApprovalUpdatedEvent = Schema.Struct({
	approval: WorkspaceGitMutationApproval,
	type: Schema.Literal("workspace.git.mutation.approval.updated"),
});

export type WorkspaceGitMutationApprovalUpdatedEvent =
	typeof WorkspaceGitMutationApprovalUpdatedEvent.Type;
