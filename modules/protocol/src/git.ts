import { Schema } from "effect";

import { Identifier, IsoDateTime, JournalSequence, PositiveInt, RawOrigin } from "./common";

const text_encoder = new TextEncoder();
const git_path_maximum_bytes = 16 * 1024;

/** Defines the maximum number of changed paths carried by one Git workspace projection. */
export const git_workspace_maximum_changed_paths = 100_000;

/** Defines the maximum number of worktrees carried by one Git workspace projection. */
export const git_workspace_maximum_worktrees = 4_096;

/** Defines the maximum number of exact paths accepted by one index mutation. */
export const git_mutation_maximum_paths = 10_000;

/** Defines the maximum number of unresolved mutations returned with a workspace query. */
export const git_workspace_maximum_pending_mutations = 10_000;

/** Defines the maximum UTF-8 size of one ephemeral Git unified diff. */
export const git_diff_maximum_bytes = 16 * 1024 * 1024;

/** Preserves an exact Git path while rejecting its NUL record delimiter. */
export const GitPath = Schema.String.check(
	Schema.makeFilter<string>((path) =>
		path.length > 0 &&
		!path.includes("\0") &&
		text_encoder.encode(path).byteLength <= git_path_maximum_bytes
			? undefined
			: `Expected a non-empty NUL-free Git path of at most ${git_path_maximum_bytes} bytes`,
	),
);

export type GitPath = typeof GitPath.Type;

/** Validates a Git SHA-1 or SHA-256 object identifier. */
export const GitObjectId = Schema.String.check(
	Schema.isPattern(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/, {
		message: "Expected a lowercase Git SHA-1 or SHA-256 object identifier",
	}),
);

export type GitObjectId = typeof GitObjectId.Type;

/** Identifies the exact content-free Git workspace state used for optimistic concurrency. */
export const GitSnapshotId = Schema.String.check(
	Schema.isPattern(/^[a-f0-9]{64}$/, {
		message: "Expected a lowercase SHA-256 Git snapshot identifier",
	}),
);

export type GitSnapshotId = typeof GitSnapshotId.Type;

/** Validates a complete local branch name using Git's ref-format restrictions. */
export const GitBranchName = Schema.String.check(
	Schema.makeFilter<string>((name) => {
		const segments = name.split("/");
		const has_invalid_character = [...name].some((character) => {
			const code = character.codePointAt(0) ?? 0;

			return code <= 32 || code === 127 || "~^:?*[\\".includes(character);
		});
		const has_invalid_segment = segments.some(
			(segment) =>
				segment.length === 0 || segment.startsWith(".") || segment.endsWith(".lock"),
		);

		return name.length === 0 ||
			text_encoder.encode(name).byteLength > 1_024 ||
			name === "@" ||
			name.startsWith("-") ||
			name.endsWith(".") ||
			name.includes("..") ||
			name.includes("@{") ||
			has_invalid_character ||
			has_invalid_segment
			? "Expected a canonical Git branch name"
			: undefined;
	}),
);

export type GitBranchName = typeof GitBranchName.Type;

/** Describes whether HEAD is attached, detached, or names an unborn branch. */
export const GitBranchState = Schema.Union([
	Schema.Struct({ name: GitBranchName, type: Schema.Literal("attached") }),
	Schema.Struct({ type: Schema.Literal("detached") }),
	Schema.Struct({ name: GitBranchName, type: Schema.Literal("unborn") }),
]);

export type GitBranchState = typeof GitBranchState.Type;

/** Carries one normalized two-column Git porcelain status. */
export const GitPorcelainStatus = Schema.String.check(
	Schema.isPattern(/^(?:[.MADRCUT]{2}|\?\?)$/, {
		message: "Expected a normalized two-column Git porcelain status",
	}),
);

export type GitPorcelainStatus = typeof GitPorcelainStatus.Type;

/** Exposes the user-facing status categories derived from Git porcelain. */
export const GitFileChangeFlags = Schema.Struct({
	conflicted: Schema.Boolean,
	staged: Schema.Boolean,
	unstaged: Schema.Boolean,
	untracked: Schema.Boolean,
});

export type GitFileChangeFlags = typeof GitFileChangeFlags.Type;

const GitFileChangeBase = Schema.Struct({
	flags: GitFileChangeFlags,
	original_path: Schema.optional(GitPath),
	path: GitPath,
	porcelain_status: GitPorcelainStatus,
});

/** Projects one changed file without carrying diff or source content. */
export const GitFileChange = GitFileChangeBase.check(
	Schema.makeFilter<typeof GitFileChangeBase.Type>((change) => {
		const status = change.porcelain_status;
		const conflicted = ["DD", "AU", "UD", "UA", "DU", "AA", "UU"].includes(status);
		const untracked = status === "??";
		const staged = !untracked && status[0] !== ".";
		const unstaged = untracked || status[1] !== ".";
		const renamed_or_copied = status.includes("R") || status.includes("C");

		return change.flags.conflicted !== conflicted ||
			change.flags.staged !== staged ||
			change.flags.unstaged !== unstaged ||
			change.flags.untracked !== untracked
			? "Expected Git change flags to match porcelain_status"
			: renamed_or_copied !== (change.original_path !== undefined)
				? "Expected original_path exactly for renamed or copied Git changes"
				: undefined;
	}),
);

export type GitFileChange = typeof GitFileChange.Type;

const GitChangedFiles = Schema.Array(GitFileChange)
	.check(Schema.isMaxLength(git_workspace_maximum_changed_paths))
	.check(
		Schema.makeFilter<ReadonlyArray<GitFileChange>>((files) =>
			new Set(files.map((file) => file.path)).size === files.length
				? undefined
				: "Expected unique Git changed paths",
		),
	);

const GitBoundedCount = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(2_147_483_647),
);

const GitBoundedFileCount = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(git_workspace_maximum_changed_paths),
);

const GitDiffSummaryBase = Schema.Struct({
	binary_file_count: GitBoundedFileCount,
	lines_added: GitBoundedCount,
	lines_deleted: GitBoundedCount,
	tracked_file_count: GitBoundedFileCount,
});

/** Summarizes tracked diff facts without carrying patch or source content. */
export const GitDiffSummary = GitDiffSummaryBase.check(
	Schema.makeFilter<typeof GitDiffSummaryBase.Type>((summary) =>
		summary.binary_file_count <= summary.tracked_file_count
			? undefined
			: "Expected binary_file_count not to exceed tracked_file_count",
	),
);

export type GitDiffSummary = typeof GitDiffSummary.Type;

/** Validates a canonical absolute path used by the read-only worktree inventory. */
export const GitWorktreePath = Schema.String.check(
	Schema.makeFilter<string>((path) => {
		const windows_root = /^[A-Z]:\/$/u.test(path);
		const windows_unc_match = /^\/\/[^/]+\/[^/]+(?:\/(.*))?$/u.exec(path);
		const windows_unc = windows_unc_match !== null;
		const posix_absolute = path.startsWith("/") && !windows_unc;
		const windows_absolute = /^[A-Z]:\//u.test(path);
		const is_absolute = posix_absolute || windows_absolute || windows_unc;
		const without_root = posix_absolute
			? path.slice(1)
			: windows_absolute
				? path.slice(3)
				: (windows_unc_match?.[1] ?? "");
		const has_invalid_segment =
			without_root.length > 0 &&
			without_root
				.split("/")
				.some((segment) => segment.length === 0 || segment === "." || segment === "..");
		const has_invalid_unc_root =
			windows_unc &&
			path
				.slice(2)
				.split("/")
				.slice(0, 2)
				.some((segment) => segment === "." || segment === "..");

		return text_encoder.encode(path).byteLength > 4_096 ||
			!is_absolute ||
			path.includes("\0") ||
			((windows_absolute || windows_unc) && /[\p{Cc}\\]/u.test(path)) ||
			(path.endsWith("/") && path !== "/" && !windows_root) ||
			has_invalid_segment ||
			has_invalid_unc_root
			? "Expected a canonical absolute Git worktree path"
			: undefined;
	}),
);

export type GitWorktreePath = typeof GitWorktreePath.Type;

const GitInventoryReason = Schema.String.check(
	Schema.makeFilter<string>((value) =>
		value.length > 0 &&
		text_encoder.encode(value).byteLength <= 4_096 &&
		!/[\p{Cc}]/u.test(value)
			? undefined
			: "Expected between 1 and 4096 UTF-8 bytes without control characters",
	),
);

const GitWorktreeBase = Schema.Struct({
	bare: Schema.Boolean,
	branch: Schema.optional(GitBranchState),
	head: Schema.optional(GitObjectId),
	is_current: Schema.Boolean,
	locked: Schema.Boolean,
	locked_reason: Schema.optional(GitInventoryReason),
	path: GitWorktreePath,
	prunable: Schema.Boolean,
	prunable_reason: Schema.optional(GitInventoryReason),
	worktree_id: Identifier,
});

/** Describes one existing Git worktree without authorizing Artisan to create one. */
export const GitWorktree = GitWorktreeBase.check(
	Schema.makeFilter<typeof GitWorktreeBase.Type>((worktree) =>
		worktree.bare &&
		(worktree.branch !== undefined || worktree.head !== undefined || worktree.is_current)
			? "Expected a bare Git worktree to have no branch, HEAD, or current-workspace authority"
			: !worktree.bare && worktree.branch === undefined
				? "Expected a non-bare Git worktree to have branch state"
				: worktree.locked_reason !== undefined && !worktree.locked
					? "Expected locked_reason only for a locked Git worktree"
					: worktree.prunable_reason !== undefined && !worktree.prunable
						? "Expected prunable_reason only for a prunable Git worktree"
						: worktree.branch?.type === "unborn" && worktree.head !== undefined
							? "Expected an unborn worktree branch to have no HEAD object identifier"
							: worktree.branch !== undefined &&
								  worktree.branch.type !== "unborn" &&
								  worktree.head === undefined
								? "Expected an attached or detached worktree branch to have a HEAD object identifier"
								: undefined,
	),
);

export type GitWorktree = typeof GitWorktree.Type;

const GitWorktrees = Schema.NonEmptyArray(GitWorktree)
	.check(Schema.isMaxLength(git_workspace_maximum_worktrees))
	.check(
		Schema.makeFilter<ReadonlyArray<GitWorktree>>((worktrees) => {
			const unique_ids = new Set(worktrees.map((worktree) => worktree.worktree_id));
			const unique_paths = new Set(worktrees.map((worktree) => worktree.path));
			const current_count = worktrees.filter((worktree) => worktree.is_current).length;

			return unique_ids.size !== worktrees.length || unique_paths.size !== worktrees.length
				? "Expected unique Git worktree identifiers and paths"
				: current_count !== 1
					? "Expected exactly one current Git worktree"
					: undefined;
		}),
	);

const GitWorkspaceProjectionBase = {
	journal_sequence: JournalSequence,
	observed_at: IsoDateTime,
	snapshot_id: GitSnapshotId,
	version: PositiveInt,
	workspace_id: Identifier,
};

/** Projects the durable observation that a workspace is not a Git repository. */
export const GitNotRepositoryProjection = Schema.Struct({
	...GitWorkspaceProjectionBase,
	repository_state: Schema.Literal("not_repository"),
});

export type GitNotRepositoryProjection = typeof GitNotRepositoryProjection.Type;

const GitRepositoryProjectionBase = Schema.Struct({
	...GitWorkspaceProjectionBase,
	aggregate: GitDiffSummary,
	branch: GitBranchState,
	clean: Schema.Boolean,
	files: GitChangedFiles,
	head: Schema.optional(GitObjectId),
	repository_state: Schema.Literal("repository"),
	staged: GitDiffSummary,
	unstaged: GitDiffSummary,
	worktrees: GitWorktrees,
});

/** Projects one complete, content-free Git workspace snapshot. */
export const GitRepositoryProjection = GitRepositoryProjectionBase.check(
	Schema.makeFilter<typeof GitRepositoryProjectionBase.Type>((workspace) =>
		workspace.clean !== (workspace.files.length === 0)
			? "Expected clean to match the changed file inventory"
			: workspace.branch.type === "unborn" && workspace.head !== undefined
				? "Expected an unborn branch to have no HEAD object identifier"
				: workspace.branch.type !== "unborn" && workspace.head === undefined
					? "Expected an attached or detached branch to have a HEAD object identifier"
					: undefined,
	),
);

export type GitRepositoryProjection = typeof GitRepositoryProjection.Type;

/** Unions every durable Git workspace observation. */
export const GitWorkspaceProjection = Schema.Union([
	GitNotRepositoryProjection,
	GitRepositoryProjection,
]);

export type GitWorkspaceProjection = typeof GitWorkspaceProjection.Type;

/** Defines the only approval-bearing Git index mutations supported by V1. */
export const GitMutationKind = Schema.Literals(["stage", "unstage"]);

export type GitMutationKind = typeof GitMutationKind.Type;

/** Defines the durable lifecycle of one approval-bearing Git mutation. */
export const GitMutationLifecycle = Schema.Literals([
	"awaiting_approval",
	"denied",
	"approved",
	"dispatching",
	"succeeded",
	"failed",
	"ambiguous",
]);

export type GitMutationLifecycle = typeof GitMutationLifecycle.Type;

const GitMutationPath = GitPath.check(
	Schema.makeFilter<GitPath>((path) => {
		const invalid_segment = path
			.split("/")
			.some((segment) => segment === "" || segment === "." || segment === "..");

		return path.startsWith("/") || /^[a-z]:/iu.test(path) || invalid_segment
			? "Expected a canonical repository-relative literal Git path"
			: undefined;
	}),
);

const GitMutationPathsBase = Schema.NonEmptyArray(GitMutationPath).check(
	Schema.isMaxLength(git_mutation_maximum_paths),
	Schema.makeFilter<ReadonlyArray<GitPath>>((paths) =>
		text_encoder.encode(`${paths.join("\0")}\0`).byteLength <= 1024 * 1024
			? undefined
			: "Expected Git mutation paths to fit the bounded subprocess input",
	),
);

/** Validates a bounded, duplicate-free list of exact Git paths. */
export const GitMutationPaths = GitMutationPathsBase.check(
	Schema.makeFilter<typeof GitMutationPathsBase.Type>((paths) =>
		new Set(paths).size === paths.length
			? undefined
			: "Expected unique exact Git mutation paths",
	),
);

export type GitMutationPaths = typeof GitMutationPaths.Type;

/** Describes a content-free terminal Git mutation failure. */
export const GitMutationFailure = Schema.Struct({
	code: Identifier,
});

export type GitMutationFailure = typeof GitMutationFailure.Type;

const GitMutationProjectionBase = Schema.Struct({
	agent_id: Schema.optional(Identifier),
	approval_id: Identifier,
	completed_at: Schema.optional(IsoDateTime),
	decision_at: Schema.optional(IsoDateTime),
	decision_message_id: Schema.optional(Identifier),
	dispatched_at: Schema.optional(IsoDateTime),
	expected_snapshot_id: GitSnapshotId,
	expected_workspace_version: PositiveInt,
	failure: Schema.optional(GitMutationFailure),
	journal_sequence: JournalSequence,
	kind: GitMutationKind,
	mutation_id: Identifier,
	paths: GitMutationPaths,
	raw_origin: Schema.optional(RawOrigin),
	requested_at: IsoDateTime,
	result_snapshot_id: Schema.optional(GitSnapshotId),
	result_workspace_version: Schema.optional(PositiveInt),
	run_id: Schema.optional(Identifier),
	source_message_id: Identifier,
	lifecycle: GitMutationLifecycle,
	thread_id: Identifier,
	updated_at: IsoDateTime,
	workspace_id: Identifier,
});

/** Projects one durable, approval-bearing Git index mutation. */
export const GitMutationProjection = GitMutationProjectionBase.check(
	Schema.makeFilter<typeof GitMutationProjectionBase.Type>((mutation) => {
		const has_any_decision =
			mutation.decision_message_id !== undefined || mutation.decision_at !== undefined;
		const has_decision =
			mutation.decision_message_id !== undefined && mutation.decision_at !== undefined;
		const has_dispatch = mutation.dispatched_at !== undefined;
		const has_completion = mutation.completed_at !== undefined;

		switch (mutation.lifecycle) {
			case "awaiting_approval":
				return has_any_decision ||
					has_dispatch ||
					has_completion ||
					mutation.failure !== undefined ||
					mutation.result_snapshot_id !== undefined ||
					mutation.result_workspace_version !== undefined
					? "Expected awaiting_approval Git mutation metadata"
					: undefined;
			case "denied":
				return !has_decision ||
					has_dispatch ||
					!has_completion ||
					mutation.failure !== undefined ||
					mutation.result_snapshot_id !== undefined ||
					mutation.result_workspace_version !== undefined
					? "Expected denied Git mutation metadata"
					: undefined;
			case "approved":
				return !has_decision ||
					has_dispatch ||
					has_completion ||
					mutation.failure !== undefined ||
					mutation.result_snapshot_id !== undefined ||
					mutation.result_workspace_version !== undefined
					? "Expected approved Git mutation metadata"
					: undefined;
			case "dispatching":
				return !has_decision ||
					!has_dispatch ||
					has_completion ||
					mutation.failure !== undefined ||
					mutation.result_snapshot_id !== undefined ||
					mutation.result_workspace_version !== undefined
					? "Expected dispatching Git mutation metadata"
					: undefined;
			case "succeeded":
				return !has_decision ||
					!has_dispatch ||
					!has_completion ||
					mutation.failure !== undefined ||
					mutation.result_snapshot_id === undefined ||
					mutation.result_workspace_version === undefined
					? "Expected succeeded Git mutation metadata"
					: undefined;
			case "failed":
			case "ambiguous":
				return !has_decision ||
					!has_dispatch ||
					!has_completion ||
					mutation.failure === undefined ||
					mutation.result_snapshot_id !== undefined ||
					mutation.result_workspace_version !== undefined
					? `Expected ${mutation.lifecycle} Git mutation metadata`
					: undefined;
		}
	}),
);

export type GitMutationProjection = typeof GitMutationProjection.Type;

/** Requests the durable Git projection and unresolved mutations for one workspace. */
export const GitWorkspaceQuery = Schema.Struct({
	thread_id: Identifier,
	workspace_id: Identifier,
});

export type GitWorkspaceQuery = typeof GitWorkspaceQuery.Type;

const GitWorkspaceQueryResultBase = Schema.Struct({
	journal_sequence: JournalSequence,
	pending_mutations: Schema.Array(GitMutationProjection).check(
		Schema.isMaxLength(git_workspace_maximum_pending_mutations),
	),
	workspace: GitWorkspaceProjection,
});

/** Returns one Git workspace projection at a durable journal position. */
export const GitWorkspaceQueryResult = GitWorkspaceQueryResultBase.check(
	Schema.makeFilter<typeof GitWorkspaceQueryResultBase.Type>((result) => {
		const mutation_ids = new Set(
			result.pending_mutations.map((mutation) => mutation.mutation_id),
		);
		const pending_lifecycles: ReadonlySet<GitMutationLifecycle> = new Set([
			"awaiting_approval",
			"approved",
			"dispatching",
			"ambiguous",
		]);

		return result.workspace.journal_sequence > result.journal_sequence ||
			result.pending_mutations.some(
				(mutation) =>
					mutation.workspace_id !== result.workspace.workspace_id ||
					mutation.journal_sequence > result.journal_sequence ||
					!pending_lifecycles.has(mutation.lifecycle),
			)
			? "Expected Git query projections to belong to the returned journal position and workspace"
			: mutation_ids.size !== result.pending_mutations.length
				? "Expected unique pending Git mutation identifiers"
				: undefined;
	}),
);

export type GitWorkspaceQueryResult = typeof GitWorkspaceQueryResult.Type;

/** Selects the tracked Git diff scope returned by a query. */
export const GitDiffScope = Schema.Literals(["staged", "unstaged", "aggregate"]);

export type GitDiffScope = typeof GitDiffScope.Type;

/** Validates a bounded unified Git patch within the V1 control-frame ceiling. */
export const GitDiffPatch = Schema.String.check(
	Schema.makeFilter<string>((patch) =>
		text_encoder.encode(patch).byteLength <= git_diff_maximum_bytes
			? undefined
			: `Expected at most ${git_diff_maximum_bytes} UTF-8 bytes`,
	),
);

export type GitDiffPatch = typeof GitDiffPatch.Type;

/** Requests one bounded Git diff for an exact observed workspace snapshot. */
export const GitDiffQuery = Schema.Struct({
	expected_snapshot_id: GitSnapshotId,
	expected_workspace_version: PositiveInt,
	max_bytes: Schema.optional(
		Schema.Int.check(
			Schema.isGreaterThan(0),
			Schema.isLessThanOrEqualTo(git_diff_maximum_bytes),
		),
	),
	scope: GitDiffScope,
	workspace_id: Identifier,
});

export type GitDiffQuery = typeof GitDiffQuery.Type;

const GitDiffQueryResultBase = Schema.Struct({
	byte_count: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(git_diff_maximum_bytes),
	),
	format: Schema.Literal("unified"),
	format_version: Schema.Literal(1),
	patch: GitDiffPatch,
	scope: GitDiffScope,
	snapshot_id: GitSnapshotId,
	truncated: Schema.Boolean,
	workspace_id: Identifier,
	workspace_version: PositiveInt,
});

/** Returns one bounded ephemeral Git diff without adding patch bytes to durable state. */
export const GitDiffQueryResult = GitDiffQueryResultBase.check(
	Schema.makeFilter<typeof GitDiffQueryResultBase.Type>((result) =>
		text_encoder.encode(result.patch).byteLength === result.byte_count
			? undefined
			: "Expected byte_count to equal the Git patch UTF-8 byte count",
	),
);

export type GitDiffQueryResult = typeof GitDiffQueryResult.Type;

const GitIndexMutationRequest = {
	approval_id: Identifier,
	expected_snapshot_id: GitSnapshotId,
	expected_workspace_version: PositiveInt,
	mutation_id: Identifier,
	paths: GitMutationPaths,
	workspace_id: Identifier,
};

/** Requests approval for staging exact paths in one observed workspace version. */
export const GitIndexStageRequest = Schema.Struct({
	...GitIndexMutationRequest,
});

export type GitIndexStageRequest = typeof GitIndexStageRequest.Type;

/** Requests approval for unstaging exact paths in one observed workspace version. */
export const GitIndexUnstageRequest = Schema.Struct({
	...GitIndexMutationRequest,
});

export type GitIndexUnstageRequest = typeof GitIndexUnstageRequest.Type;

/** Resolves the approval bound to one exact Git mutation. */
export const GitMutationResolveRequest = Schema.Struct({
	approval_id: Identifier,
	approved: Schema.Boolean,
	mutation_id: Identifier,
});

export type GitMutationResolveRequest = typeof GitMutationResolveRequest.Type;

/** Announces a durable Git workspace projection replacement. */
export const GitWorkspaceUpdatedEvent = Schema.Struct({
	cause: Schema.Literals(["refresh", "mutation", "recovery"]),
	type: Schema.Literal("git.workspace.updated"),
	workspace: GitWorkspaceProjection,
});

export type GitWorkspaceUpdatedEvent = typeof GitWorkspaceUpdatedEvent.Type;

/** Announces one durable Git mutation lifecycle transition. */
export const GitMutationUpdatedEvent = Schema.Struct({
	mutation: GitMutationProjection,
	type: Schema.Literal("git.mutation.updated"),
});

export type GitMutationUpdatedEvent = typeof GitMutationUpdatedEvent.Type;
