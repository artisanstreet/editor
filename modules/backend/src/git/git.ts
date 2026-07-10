import { Context, Data, Effect, Option } from "effect";

/** Describes a Git operation that failed. */
export type GitOperation = "configuration" | "diff" | "discover" | "status";

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
		readonly Status: Effect.Effect<ReadonlyArray<GitFileSummary>, GitError>;
	}
>()("Artisan/Git") {}
