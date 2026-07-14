import { Context, Data, Effect, Schema } from "effect";

import {
	GitCommitMessage,
	GitLocalBranchName,
	GitObjectId,
	GitRemoteName,
	GitResetMode,
	WorkspaceGitMutationOperation,
	WorkspaceGitMutationRejectionReason,
} from "@artisan/protocol";

const text_encoder = new TextEncoder();

const BoundedText = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		text_encoder.encode(value).byteLength > 64 * 1024 || value.includes("\0")
			? "Expected bounded text without NUL"
			: undefined,
	),
);

const GitCleanCandidates = Schema.Array(BoundedText).check(
	Schema.makeFilter<ReadonlyArray<string>>((candidates) => {
		const byte_count = candidates.reduce(
			(total, candidate) => total + text_encoder.encode(candidate).byteLength + 1,
			0,
		);

		return candidates.length > 4_096 || byte_count > 64 * 1024
			? "Expected at most 4096 clean candidates using at most 64 KiB"
			: undefined;
	}),
);

const Digest = Schema.String.check(Schema.isPattern(/^[a-f0-9]{64}$/u));
const GitState = Schema.Literals(["none", "merge", "rebase"]);

function is_supported_remote_endpoint(value: string) {
	if (/^(?:[A-Za-z]:[\\/]|\\\\|\/)/u.test(value)) {
		return true;
	}

	if (/^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+:[A-Za-z0-9._~/-]+$/u.test(value)) {
		return true;
	}

	try {
		const endpoint = new URL(value);
		const has_ambiguous_suffix = endpoint.hash.length > 0 || endpoint.search.length > 0;

		if (endpoint.protocol === "file:") {
			return (
				endpoint.username.length === 0 &&
				endpoint.password.length === 0 &&
				!has_ambiguous_suffix
			);
		}

		if (endpoint.protocol === "https:") {
			return (
				endpoint.hostname.length > 0 &&
				endpoint.username.length === 0 &&
				endpoint.password.length === 0 &&
				!has_ambiguous_suffix
			);
		}

		return (
			endpoint.protocol === "ssh:" &&
			endpoint.hostname.length > 0 &&
			endpoint.password.length === 0 &&
			!has_ambiguous_suffix
		);
	} catch {
		return false;
	}
}

export const GitRemoteEndpoint = BoundedText.check(
	Schema.makeFilter<string>((value) => {
		const byte_count = text_encoder.encode(value).byteLength;

		return value.length === 0 ||
			byte_count > 4096 ||
			/[\r\n]/u.test(value) ||
			!is_supported_remote_endpoint(value)
			? "Expected a bounded single-line Git remote endpoint"
			: undefined;
	}),
);

const GitMutationSourceProof = Schema.Struct({
	branch: Schema.optional(GitLocalBranchName),
	configuration_identity: Digest,
	head: GitObjectId,
	index_identity: Digest,
	repository_identity: Digest,
	state: GitState,
	state_identity: Digest,
	status_identity: Digest,
	tracked_identity: Digest,
	untracked_identity: Digest,
	worktree_identity: Digest,
});

/** Captures the complete bounded source observation behind one approval. */
export type GitMutationSourceProof = typeof GitMutationSourceProof.Type;

/** Carries the private conflict state that authorizes one continuation action. */
export const GitMutationActionAnchor = Schema.Struct({
	branch: GitLocalBranchName,
	identity: Digest,
	original_head: GitObjectId,
	plan_binding: Digest,
	state: GitState,
	target_head: GitObjectId,
	type: Schema.Literals(["merge", "rebase"]),
});
export type GitMutationActionAnchor = typeof GitMutationActionAnchor.Type;

const GitMutationPlanBase = {
	binding: Digest,
	source: GitMutationSourceProof,
};

/** A closed backend-private, JSON-safe plan whose sensitive fields never enter projections. */
export const GitMutationPlan = Schema.Union([
	Schema.Struct({
		...GitMutationPlanBase,
		branch: GitLocalBranchName,
		source_head: GitObjectId,
		type: Schema.Literal("branch_create"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		target_branch: GitLocalBranchName,
		target_head: GitObjectId,
		type: Schema.Literal("checkout"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		mode: GitResetMode,
		target: GitObjectId,
		type: Schema.Literal("reset"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		candidates: GitCleanCandidates,
		inventory_identity: Digest,
		type: Schema.Literal("clean"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		message: GitCommitMessage,
		type: Schema.Literal("commit"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		action: Schema.Literal("start"),
		target_branch: GitLocalBranchName,
		target_head: GitObjectId,
		type: Schema.Literal("merge"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		action: Schema.Literals(["continue", "abort"]),
		anchor: GitMutationActionAnchor,
		type: Schema.Literal("merge"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		action: Schema.Literal("start"),
		target_branch: GitLocalBranchName,
		target_head: GitObjectId,
		type: Schema.Literal("rebase"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		action: Schema.Literals(["continue", "abort", "skip"]),
		anchor: GitMutationActionAnchor,
		type: Schema.Literal("rebase"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		remote: GitRemoteName,
		remote_endpoint: GitRemoteEndpoint,
		target_branch: GitLocalBranchName,
		tracking_head: Schema.optional(GitObjectId),
		upstream_head: GitObjectId,
		type: Schema.Literal("pull_ff_only"),
	}),
	Schema.Struct({
		...GitMutationPlanBase,
		expected_remote_head: Schema.optional(GitObjectId),
		remote: GitRemoteName,
		remote_endpoint: GitRemoteEndpoint,
		set_upstream: Schema.Boolean,
		source_branch: GitLocalBranchName,
		source_head: GitObjectId,
		target_branch: GitLocalBranchName,
		tracking_head: Schema.optional(GitObjectId),
		type: Schema.Literal("push"),
	}),
]);
export type GitMutationPlan = typeof GitMutationPlan.Type;

/** Records an execution phase without exposing command output or the workspace root. */
export const GitMutationAttempt = Schema.Struct({
	binding: Digest,
	exit_code: Schema.Int,
	operation_head: Schema.optional(GitObjectId),
	output_complete: Schema.Boolean,
	output_identity: Digest,
	phase: Schema.Literals(["precondition", "mutation", "settlement"]),
	plan_binding: Digest,
	rejection_reason: Schema.optional(WorkspaceGitMutationRejectionReason),
	result: Schema.optional(GitMutationSourceProof),
	type: Schema.Literal("attempt"),
});
export type GitMutationAttempt = typeof GitMutationAttempt.Type;

/** Reports the only durable outcomes a coordinator may safely consume. */
export const GitMutationReconciliation = Schema.Union([
	Schema.Struct({ type: Schema.Literal("source") }),
	Schema.Struct({
		branch: Schema.optional(GitLocalBranchName),
		head: GitObjectId,
		remote_head: Schema.optional(GitObjectId),
		type: Schema.Literal("applied"),
	}),
	Schema.Struct({
		action: Schema.Literals(["merge_conflict", "rebase_conflict"]),
		anchor: GitMutationActionAnchor,
		type: Schema.Literal("action_required"),
	}),
	Schema.Struct({
		reason: WorkspaceGitMutationRejectionReason,
		type: Schema.Literal("rejected"),
	}),
	Schema.Struct({ type: Schema.Literal("outcome_unknown") }),
]);
export type GitMutationReconciliation = typeof GitMutationReconciliation.Type;

/** Allows a later coordinator to carry an exact private conflict anchor into preparation. */
export const GitMutationPreparation = Schema.Union([
	WorkspaceGitMutationOperation,
	Schema.Struct({
		action_anchor: GitMutationActionAnchor,
		operation: WorkspaceGitMutationOperation,
	}),
]);
export type GitMutationPreparation = typeof GitMutationPreparation.Type;

/** Reports invalid plans, stale proofs, or unavailable Git execution without public output. */
export class GitMutationError extends Data.TaggedError("GitMutationError")<{
	readonly cause?: unknown;
	readonly operation:
		| "checkout"
		| "configuration"
		| "integrity"
		| "invalid_plan"
		| "prepare"
		| "precondition"
		| "process"
		| "reconcile";
}> {}

/** Provides guarded local Git planning, execution, and receipt-bound recovery. */
export class GitMutation extends Context.Service<
	GitMutation,
	{
		readonly Prepare: (operation: unknown) => Effect.Effect<GitMutationPlan, GitMutationError>;
		readonly Execute: (plan: unknown) => Effect.Effect<GitMutationAttempt, GitMutationError>;
		readonly Reconcile: (
			plan: unknown,
			attempt?: unknown,
		) => Effect.Effect<GitMutationReconciliation, GitMutationError>;
	}
>()("Artisan/GitMutation") {}
