import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";
import { GitBranchState, GitObjectId } from "./git";

/**
 * Names the hosting service a remote points at. Detection is structural — it
 * reads the remote's host name — so a self-hosted GitLab or Gitea is recognised
 * the same way the public services are. `other` covers a reachable web host
 * that matches no known service; `unknown` covers a remote whose URL carries no
 * usable host at all, such as a bare filesystem path.
 *
 * @since 0.8.0
 */
export const RepositoryHost = Schema.Literals([
	"azure",
	"bitbucket",
	"codeberg",
	"gitea",
	"github",
	"gitlab",
	"other",
	"sourcehut",
	"unknown",
]);

export type RepositoryHost = typeof RepositoryHost.Type;

/**
 * Projects one configured remote. `web_url` is present only when the remote
 * resolves to an `https` page a browser can open — an `ssh://` or `scp`-style
 * remote is translated, while a filesystem remote yields none.
 *
 * @since 0.8.0
 */
export const RepositoryRemote = Schema.Struct({
	host: RepositoryHost,
	name: Schema.NonEmptyString,
	/** The remote's own URL, exactly as Git reports it. */
	url: Schema.NonEmptyString,
	web_url: Schema.optional(Schema.NonEmptyString),
});

export type RepositoryRemote = typeof RepositoryRemote.Type;

const repository_maximum_remotes = 64;

/** Projects a directory that Git does not track. */
export const NotRepositorySnapshot = Schema.Struct({
	state: Schema.Literal("not_repository"),
});

export type NotRepositorySnapshot = typeof NotRepositorySnapshot.Type;

const RepositorySnapshotBase = Schema.Struct({
	branch: GitBranchState,
	/**
	 * The remote a link should target: `origin` when present, otherwise the
	 * first configured remote. Absent for a repository with no remotes at all,
	 * which is the local-only case.
	 */
	default_remote: Schema.optional(Schema.NonEmptyString),
	head: Schema.optional(GitObjectId),
	remotes: Schema.Array(RepositoryRemote).check(Schema.isMaxLength(repository_maximum_remotes)),
	state: Schema.Literal("repository"),
});

/**
 * Projects one repository's identity: where HEAD sits and where it publishes.
 *
 * Deliberately free of working-tree content. Diff, commit, and branch listings
 * belong to their own queries against this same module rather than being folded
 * in here, so a caller that only wants a branch name does not pay for a status
 * walk.
 *
 * @since 0.8.0
 */
export const RepositorySnapshot = RepositorySnapshotBase.check(
	Schema.makeFilter<typeof RepositorySnapshotBase.Type>((snapshot) =>
		snapshot.branch.type === "unborn" && snapshot.head !== undefined
			? "Expected an unborn branch to have no HEAD object identifier"
			: snapshot.default_remote !== undefined &&
				  !snapshot.remotes.some((remote) => remote.name === snapshot.default_remote)
				? "Expected the default remote to name a configured remote"
				: snapshot.default_remote === undefined && snapshot.remotes.length > 0
					? "Expected a repository with remotes to name a default remote"
					: undefined,
	),
);

export type RepositorySnapshot = typeof RepositorySnapshot.Type;

/** Unions every observation of a project's repository state. */
export const ProjectRepository = Schema.Union([NotRepositorySnapshot, RepositorySnapshot]);

export type ProjectRepository = typeof ProjectRepository.Type;

/** Pairs one project with the repository observed at its root. */
export const ProjectRepositoryEntry = Schema.Struct({
	project_id: Identifier,
	repository: ProjectRepository,
});

export type ProjectRepositoryEntry = typeof ProjectRepositoryEntry.Type;

const project_repository_maximum_projects = 128;

/** Exported so a producer can bound its projection to what the result accepts. */
export const ProjectDiffMaximumProjects = project_repository_maximum_projects;

/**
 * Requests repository state for named projects. Empty `project_ids` asks for
 * every project in the catalog, which is what a picker listing them all wants.
 *
 * @since 0.8.0
 */
export const ProjectRepositoryQuery = Schema.Struct({
	project_ids: Schema.Array(Identifier).check(
		Schema.isMaxLength(project_repository_maximum_projects),
	),
});

export type ProjectRepositoryQuery = typeof ProjectRepositoryQuery.Type;

/**
 * Returns repository state per project. A root that has moved or lost its
 * repository reports `not_repository` rather than failing the whole query.
 *
 * @since 0.8.0
 */
export const ProjectRepositoryQueryResult = Schema.Struct({
	repositories: Schema.Array(ProjectRepositoryEntry).check(
		Schema.isMaxLength(project_repository_maximum_projects),
	),
});

export type ProjectRepositoryQueryResult = typeof ProjectRepositoryQueryResult.Type;

const DiffBoundedCount = Schema.Int.check(
	Schema.isGreaterThanOrEqualTo(0),
	Schema.isLessThanOrEqualTo(2_147_483_647),
);

const RepositoryDiffCountsBase = Schema.Struct({
	/** Binary files carry no line counts, so they are reported as a count of their own. */
	binary_file_count: DiffBoundedCount,
	file_count: DiffBoundedCount,
	lines_added: DiffBoundedCount,
	lines_deleted: DiffBoundedCount,
});

/** Counts one diff without carrying any patch content. */
export const RepositoryDiffCounts = RepositoryDiffCountsBase.check(
	Schema.makeFilter<typeof RepositoryDiffCountsBase.Type>((counts) =>
		counts.binary_file_count > counts.file_count
			? "Expected binary_file_count not to exceed file_count"
			: undefined,
	),
);

export type RepositoryDiffCounts = typeof RepositoryDiffCounts.Type;

/**
 * Compares the checked-out branch against another ref.
 *
 * `counts` measures the merge base against `HEAD` — the branch's own committed
 * work, excluding the working tree, which is reported separately. `ahead` and
 * `behind` count commits either side of that same merge base.
 *
 * @since 0.8.0
 */
export const RepositoryBranchComparison = Schema.Struct({
	ahead: DiffBoundedCount,
	behind: DiffBoundedCount,
	counts: RepositoryDiffCounts,
	/**
	 * `upstream` is the branch's own tracking ref; `default_branch` is where the
	 * remote points its own HEAD, which is the branch a pull request targets.
	 */
	kind: Schema.Literals(["default_branch", "upstream"]),
	ref: Schema.NonEmptyString,
});

export type RepositoryBranchComparison = typeof RepositoryBranchComparison.Type;

const repository_maximum_comparisons = 4;

const RepositoryDiffSnapshotBase = Schema.Struct({
	/** Absent entirely when a ref cannot be resolved, rather than reported as zero. */
	comparisons: Schema.Array(RepositoryBranchComparison).check(
		Schema.isMaxLength(repository_maximum_comparisons),
	),
	head_committed_at: Schema.optional(IsoDateTime),
	/** The index against `HEAD`: work that a commit would take. */
	staged: RepositoryDiffCounts,
	state: Schema.Literal("repository"),
	stash_count: DiffBoundedCount,
	/**
	 * True when a read hit its byte cap or its tail could not be parsed, so the
	 * counts are a floor rather than a total. A reader must say so: presenting a
	 * partial count as complete turns "ten thousand files changed" into "clean".
	 */
	truncated: Schema.Boolean,
	/** The working tree against the index: work a commit would leave behind. */
	unstaged: RepositoryDiffCounts,
	/**
	 * Untracked files are outside `git diff`'s reach, so their lines are absent
	 * from every count here and only the file tally is reported. A reader that
	 * shows `+0 −0` beside a non-zero tally would be describing new files as no
	 * change, so it must surface this separately.
	 */
	untracked_file_count: DiffBoundedCount,
	/** The whole working tree, index included, against `HEAD`. */
	working: RepositoryDiffCounts,
});

/**
 * Summarizes a project's state against every baseline that resolves.
 *
 * `working` leads because `HEAD` is the only baseline always defined — it needs
 * no upstream, no fetch, and no guess at a default branch, and it still answers
 * on a detached or unborn branch. The branch comparisons are best-effort by
 * nature: an unpushed branch has no upstream, and a remote whose own HEAD was
 * never fetched names no default branch, so each is present only when its ref
 * resolves.
 *
 * @since 0.8.0
 */
export const RepositoryDiffSnapshot = RepositoryDiffSnapshotBase.check(
	Schema.makeFilter<typeof RepositoryDiffSnapshotBase.Type>((snapshot) =>
		new Set(snapshot.comparisons.map((comparison) => comparison.kind)).size !==
		snapshot.comparisons.length
			? "Expected at most one comparison per kind"
			: undefined,
	),
);

export type RepositoryDiffSnapshot = typeof RepositoryDiffSnapshot.Type;

/** Unions every observation of a project's uncommitted diff. */
export const ProjectDiff = Schema.Union([NotRepositorySnapshot, RepositoryDiffSnapshot]);

export type ProjectDiff = typeof ProjectDiff.Type;

/** Pairs one project with the diff observed at its root. */
export const ProjectDiffEntry = Schema.Struct({
	diff: ProjectDiff,
	project_id: Identifier,
});

export type ProjectDiffEntry = typeof ProjectDiffEntry.Type;

/**
 * Requests uncommitted diff summaries for named projects. Empty `project_ids`
 * asks for every project, which costs one status walk each — a caller showing a
 * single project should name it.
 *
 * @since 0.8.0
 */
export const ProjectDiffQuery = Schema.Struct({
	project_ids: Schema.Array(Identifier).check(Schema.isMaxLength(ProjectDiffMaximumProjects)),
});

export type ProjectDiffQuery = typeof ProjectDiffQuery.Type;

/**
 * Returns diff state per project. A root that has moved or lost its repository
 * reports `not_repository` rather than failing the whole query, and a catalog
 * larger than the bound is reported up to it rather than overflowing.
 *
 * @since 0.8.0
 */
export const ProjectDiffQueryResult = Schema.Struct({
	diffs: Schema.Array(ProjectDiffEntry).check(Schema.isMaxLength(ProjectDiffMaximumProjects)),
});

export type ProjectDiffQueryResult = typeof ProjectDiffQueryResult.Type;
