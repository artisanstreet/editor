import { Context, Data, Effect, FileSystem, Layer, Option } from "effect";

import type {
	GitDiffStats as GitDiffStatsValue,
	WorkspaceGitChangedFile,
	WorkspaceGitSessionBlocker,
	WorkspaceGitSessionState,
	WorkspaceGitWorktree,
} from "@artisan/protocol";

import type { GitFileSummary, GitRepository, GitWorktree } from "./git";
import { WorkspaceGitRegistry } from "./workspace-git-registry";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

/** Keeps one private adapter path beside the renderer-safe worktree facts. */
export interface WorkspaceGitObservedWorktree extends WorkspaceGitWorktree {
	readonly adapter_path: string;
}

/** Carries one bounded Git observation before persistence assigns its version and cursor. */
export interface WorkspaceGitObservation {
	readonly adapter_worktrees: ReadonlyArray<WorkspaceGitObservedWorktree>;
	readonly blockers: ReadonlyArray<WorkspaceGitSessionBlocker>;
	readonly branch?: string;
	readonly changed_files: ReadonlyArray<WorkspaceGitChangedFile>;
	readonly diff_stats: GitDiffStatsValue;
	readonly has_diff: boolean;
	readonly head?: string;
	readonly observed_at: string;
	readonly repository_root?: string;
	readonly selected_worktree_path?: string;
	readonly state: WorkspaceGitSessionState;
	readonly worktrees: ReadonlyArray<WorkspaceGitWorktree>;
	readonly workspace_id: string;
}

/** Conceals native Git and path failures behind one session-observation boundary. */
export class WorkspaceGitObservationError extends Data.TaggedError("WorkspaceGitObservationError")<{
	readonly cause?: unknown;
	readonly reason: "git_failed" | "invalid_state" | "path_failed" | "workspace_unavailable";
}> {}

/** Observes one registered checkout without mutating Git or creating worktrees. */
export class WorkspaceGitObserver extends Context.Service<
	WorkspaceGitObserver,
	{
		readonly Observe: (
			workspace_id: string,
		) => Effect.Effect<WorkspaceGitObservation, WorkspaceGitObservationError>;
	}
>()("Artisan/WorkspaceGitObserver") {}

function observation_error(reason: WorkspaceGitObservationError["reason"], cause?: unknown) {
	return new WorkspaceGitObservationError({
		...(cause === undefined ? {} : { cause }),
		reason,
	});
}

function branch_name(reference: string) {
	const prefix = "refs/heads/";

	return reference.startsWith(prefix) ? reference.slice(prefix.length) : reference;
}

function optional_value<A>(value: Option.Option<A>) {
	return Option.isSome(value) ? value.value : undefined;
}

function repositories_match(left: GitRepository, right: GitRepository) {
	return (
		left.branch === right.branch &&
		optional_value(left.head) === optional_value(right.head) &&
		left.root === right.root
	);
}

function changed_file_from(summary: GitFileSummary): WorkspaceGitChangedFile {
	return {
		conflicted: summary.conflicted,
		...(summary.original_path === undefined ? {} : { original_path: summary.original_path }),
		path: summary.path,
		staged: summary.staged,
		status: summary.status,
		untracked: summary.untracked,
		unstaged: summary.unstaged,
	};
}

function public_worktree(
	worktree: GitWorktree,
	canonical_path: string | undefined,
	selected_root: string,
): WorkspaceGitObservedWorktree {
	const branch = optional_value(worktree.branch);
	const head = optional_value(worktree.head);

	return {
		adapter_path: canonical_path ?? worktree.adapter_path,
		bare: worktree.bare,
		...(branch === undefined ? {} : { branch: branch_name(branch) }),
		detached: worktree.detached,
		...(head === undefined ? {} : { head }),
		locked: worktree.locked,
		location: canonical_path === selected_root ? "selected" : "external",
		prunable: worktree.prunable,
	};
}

/** Supplies the read-only observer over registered Git capabilities and Effect FileSystem. */
export const WorkspaceGitObserverLive = Layer.effect(
	WorkspaceGitObserver,
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const metadata = yield* RuntimeMetadata;
		const registry = yield* WorkspaceGitRegistry;

		const Observe = (workspace_id: string) =>
			Effect.gen(function* () {
				const capability = yield* registry
					.Get(workspace_id)
					.pipe(
						Effect.mapError((cause) =>
							observation_error("workspace_unavailable", cause),
						),
					);
				const repository = yield* capability.read.ProbeRepository.pipe(
					Effect.mapError((cause) => observation_error("git_failed", cause)),
				);
				const observed_at = yield* metadata.Now;

				if (Option.isNone(repository)) {
					return {
						adapter_worktrees: [],
						blockers: ["not_repository"],
						changed_files: [],
						diff_stats: { additions: 0, deletions: 0, files: 0 },
						has_diff: false,
						observed_at,
						state: "unavailable",
						worktrees: [],
						workspace_id,
					} satisfies WorkspaceGitObservation;
				}

				const first_repository = repository.value;
				const observed = yield* Effect.all({
					diff_stats: capability.read.DiffStats,
					status: capability.read.Status,
					worktrees: capability.read.Worktrees,
				}).pipe(Effect.mapError((cause) => observation_error("git_failed", cause)));
				const second_repository = yield* capability.read.Discover.pipe(
					Effect.mapError((cause) => observation_error("git_failed", cause)),
				);

				if (!repositories_match(first_repository, second_repository)) {
					return yield* Effect.fail(observation_error("invalid_state"));
				}

				const repository_root = yield* file_system
					.realPath(second_repository.root)
					.pipe(Effect.mapError((cause) => observation_error("path_failed", cause)));
				const canonical_worktree_paths = yield* Effect.forEach(
					observed.worktrees,
					(worktree) => file_system.realPath(worktree.adapter_path).pipe(Effect.option),
				);
				const adapter_worktrees = observed.worktrees
					.map((worktree, index) =>
						public_worktree(
							worktree,
							Option.getOrUndefined(canonical_worktree_paths[index]!),
							capability.canonical_root,
						),
					)
					.toSorted((left, right) => left.adapter_path.localeCompare(right.adapter_path));
				const worktrees = adapter_worktrees.map(
					({ adapter_path: _adapter_path, ...worktree }) => worktree,
				);
				const selected = worktrees.filter((worktree) => worktree.location === "selected");
				const blockers = [
					...(selected.length === 0 ? (["selected_worktree_missing"] as const) : []),
					...(worktrees.length > 1 ? (["multiple_worktrees"] as const) : []),
					...(worktrees.some((worktree) => worktree.bare)
						? (["bare_repository"] as const)
						: []),
					...(second_repository.branch.length === 0 ||
					worktrees.some((worktree) => worktree.detached)
						? (["detached_head"] as const)
						: []),
					...(worktrees.some((worktree) => worktree.locked)
						? (["locked_worktree"] as const)
						: []),
					...(worktrees.some((worktree) => worktree.prunable)
						? (["prunable_worktree"] as const)
						: []),
					...(repository_root !== capability.canonical_root
						? (["selected_worktree_mismatch"] as const)
						: []),
					...(Option.isNone(second_repository.head) ? (["unborn_head"] as const) : []),
				] satisfies ReadonlyArray<WorkspaceGitSessionBlocker>;
				const branch =
					second_repository.branch.length === 0 ? undefined : second_repository.branch;
				const head = optional_value(second_repository.head);
				const changed_files = observed.status
					.map(changed_file_from)
					.toSorted((left, right) => left.path.localeCompare(right.path));

				return {
					adapter_worktrees,
					blockers,
					...(branch === undefined ? {} : { branch }),
					changed_files,
					diff_stats: observed.diff_stats,
					has_diff: changed_files.length > 0,
					...(head === undefined ? {} : { head }),
					observed_at,
					repository_root,
					selected_worktree_path: capability.canonical_root,
					state: blockers.length === 0 ? "ready" : "blocked",
					worktrees,
					workspace_id,
				} satisfies WorkspaceGitObservation;
			});

		return { Observe };
	}),
);
