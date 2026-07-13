import { Context, Data, Effect, Option } from "effect";

/** Describes a Git operation that failed. */
export type GitOperation =
	| "configuration"
	| "diff"
	| "discover"
	| "probe"
	| "resolve_branch"
	| "status"
	| "worktrees";

/** Reports a Git failure without exposing process implementation details. */
export class GitError extends Data.TaggedError("GitError")<{
	readonly cause: unknown;
	readonly operation: GitOperation;
}> {}

/** Describes the repository and current branch state. */
export interface GitRepository {
	readonly branch: string;
	readonly head: Option.Option<string>;
	readonly root: string;
}

/** Describes one Git worktree reported by the configured adapter. */
export interface GitWorktree {
	readonly adapter_path: string;
	readonly bare: boolean;
	readonly branch: Option.Option<string>;
	readonly detached: boolean;
	readonly head: Option.Option<string>;
	readonly locked: boolean;
	readonly prunable: boolean;
}

/** Summarizes one changed path in the working tree. */
export interface GitFileSummary {
	readonly conflicted: boolean;
	readonly original_path?: string;
	readonly path: string;
	readonly staged: boolean;
	readonly status: string;
	readonly untracked: boolean;
	readonly unstaged: boolean;
}

/** Reports aggregate diff size. */
export interface GitDiffStats {
	readonly additions: number;
	readonly deletions: number;
	readonly files: number;
}

/** Contains a UTF-8-safe bounded patch and its truncation metadata. */
export interface GitDiffPatch {
	readonly bytes: number;
	readonly patch: string;
	readonly truncated: boolean;
}

/** Provides bounded, provider-neutral Git read operations. */
export class Git extends Context.Service<
	Git,
	{
		readonly DiffPatch: (max_bytes?: number) => Effect.Effect<GitDiffPatch, GitError>;
		readonly DiffStats: Effect.Effect<GitDiffStats, GitError>;
		readonly Discover: Effect.Effect<GitRepository, GitError>;
		readonly ProbeRepository: Effect.Effect<Option.Option<GitRepository>, GitError>;
		readonly ResolveLocalBranch: (
			branch: string,
		) => Effect.Effect<Option.Option<string>, GitError>;
		readonly Status: Effect.Effect<ReadonlyArray<GitFileSummary>, GitError>;
		readonly Worktrees: Effect.Effect<ReadonlyArray<GitWorktree>, GitError>;
	}
>()("Artisan/Git") {}
