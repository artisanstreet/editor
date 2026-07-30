import { Schema } from "effect";

const git_path_maximum_bytes = 16 * 1024;
const text_encoder = new TextEncoder();

/** Validates one Git object identifier without assuming SHA-1 forever. */
export const GitObjectId = Schema.String.check(
	Schema.isPattern(/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/u, {
		message: "Expected a lowercase SHA-1 or SHA-256 Git object identifier",
	}),
);

export type GitObjectId = typeof GitObjectId.Type;

/** Preserves odd but valid Git paths while rejecting the NUL record delimiter. */
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

export const GitAttachedHead = Schema.Struct({
	_tag: Schema.Literal("attached"),
	branch: Schema.NonEmptyString,
	oid: GitObjectId,
});

export const GitDetachedHead = Schema.Struct({
	_tag: Schema.Literal("detached"),
	oid: GitObjectId,
});

export const GitUnbornHead = Schema.Struct({
	_tag: Schema.Literal("unborn"),
	branch: Schema.NonEmptyString,
});

/** Represents attached, detached, and initial repositories without sentinel strings. */
export const GitHead = Schema.Union([GitAttachedHead, GitDetachedHead, GitUnbornHead]);

export type GitHead = typeof GitHead.Type;

export const GitNoUpstream = Schema.Struct({
	_tag: Schema.Literal("none"),
});

export const GitTrackedUpstream = Schema.Struct({
	_tag: Schema.Literal("tracked"),
	ahead: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	behind: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	ref: Schema.NonEmptyString,
});

/** Represents whether the current attached branch tracks another ref. */
export const GitUpstream = Schema.Union([GitNoUpstream, GitTrackedUpstream]);

export type GitUpstream = typeof GitUpstream.Type;

export const GitFileKind = Schema.Literals([
	"ordinary",
	"renamed",
	"unmerged",
	"untracked",
	"ignored",
]);

export type GitFileKind = typeof GitFileKind.Type;

/** Projects one porcelain-v2 path without discarding its original XY state. */
export const GitFileStatus = Schema.Struct({
	conflicted: Schema.Boolean,
	index_status: Schema.Char,
	kind: GitFileKind,
	original_path: Schema.optional(GitPath),
	path: GitPath,
	staged: Schema.Boolean,
	status: Schema.String.check(Schema.isLengthBetween(2, 2)),
	submodule: Schema.optional(Schema.NonEmptyString),
	untracked: Schema.Boolean,
	unstaged: Schema.Boolean,
	worktree_status: Schema.Char,
});

export type GitFileStatus = typeof GitFileStatus.Type;

/** Contains aggregate tracked-file diff statistics from `--numstat -z`. */
export const GitDiffStats = Schema.Struct({
	additions: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	binary_files: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	deletions: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	files: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
});

export type GitDiffStats = typeof GitDiffStats.Type;

/** Describes one checkout reported by `git worktree list --porcelain -z`. */
export const GitWorktree = Schema.Struct({
	bare: Schema.Boolean,
	branch: Schema.optional(Schema.NonEmptyString),
	current: Schema.Boolean,
	detached: Schema.Boolean,
	head: Schema.optional(GitObjectId),
	locked_reason: Schema.optional(Schema.String),
	path: GitPath,
	prunable_reason: Schema.optional(Schema.String),
});

export type GitWorktree = typeof GitWorktree.Type;

/** Carries the branch metadata and changed paths parsed from one status command. */
export const GitParsedStatus = Schema.Struct({
	files: Schema.Array(GitFileStatus),
	head: GitHead,
	upstream: GitUpstream,
});

export type GitParsedStatus = typeof GitParsedStatus.Type;

/** Represents one coherently sampled repository status for an opaque workspace. */
export const GitStatusSnapshot = Schema.Struct({
	aggregate: GitDiffStats,
	files: Schema.Array(GitFileStatus),
	head: GitHead,
	root: GitPath,
	snapshot_id: Schema.String.check(
		Schema.isPattern(/^[a-f0-9]{64}$/u, {
			message: "Expected a lowercase SHA-256 Git snapshot identifier",
		}),
	),
	staged: GitDiffStats,
	unstaged: GitDiffStats,
	upstream: GitUpstream,
	workspace_id: Schema.NonEmptyString,
	worktrees: Schema.Array(GitWorktree),
});

export type GitStatusSnapshot = typeof GitStatusSnapshot.Type;

/** Selects the tracked diff relation used for one bounded patch. */
export const GitDiffScope = Schema.Literals(["all", "staged", "unstaged"]);

export type GitDiffScope = typeof GitDiffScope.Type;

/** Requests one bounded patch from a registered workspace snapshot. */
export const GitPatchRequest = Schema.Struct({
	max_bytes: Schema.optional(
		Schema.Int.check(
			Schema.isGreaterThanOrEqualTo(0),
			Schema.isLessThanOrEqualTo(32 * 1024 * 1024),
		),
	),
	scope: GitDiffScope,
	workspace_id: Schema.NonEmptyString,
});

export type GitPatchRequest = typeof GitPatchRequest.Type;

/** Contains a bounded UTF-8 patch and explicit truncation metadata. */
export const GitDiffPatch = Schema.Struct({
	bytes: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
	patch: Schema.String,
	truncated: Schema.Boolean,
});

export type GitDiffPatch = typeof GitDiffPatch.Type;
