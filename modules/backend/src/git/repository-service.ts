import { Context, Data, Effect, Layer } from "effect";

import type {
	GitBranchState,
	ProjectDiff,
	ProjectRepository,
	RepositoryBranchComparison,
	RepositoryDiffCounts,
	RepositoryRemote,
} from "@artisan/protocol";

import { GitCommandExecutor, type GitCommandResult } from "./executor";
import { ParseGitNumstat } from "./parsers";
import { RepositoryHostFor, RepositoryWebUrlFor } from "./remote-url";

/**
 * Reads repository identity for an arbitrary directory.
 *
 * Distinct from the workspace Git projection, which is thread-scoped, durable,
 * and status-oriented. This answers "what repository is this directory, and
 * where does it publish" for any path Forge knows about — the question a
 * project picker asks. Commit, branch, and diff reads belong beside `Inspect`
 * as this module grows; each stays its own call so a caller pays only for what
 * it reads.
 *
 * @since 0.8.0
 */

const decoder = new TextDecoder();

/** Git output is bounded well below the executor's ceiling; these are small reads. */
const max_stdout_bytes = 256 * 1024;
/**
 * Numstat and untracked listings scale with the working tree, not with identity.
 * 4 MiB holds roughly a hundred thousand records, past which the reading reports
 * itself truncated rather than pretending to be complete.
 */
const max_diff_stdout_bytes = 4 * 1024 * 1024;
const max_stderr_bytes = 16 * 1024;
const maximum_remotes = 64;

export type RepositoryReadOperation =
	| "ahead_behind"
	| "branch"
	| "default_branch"
	| "diff"
	| "discover"
	| "head"
	| "remotes"
	| "stash"
	| "untracked"
	| "upstream";

/** Conceals executor failures behind a repository-specific infrastructure error. */
export class RepositoryServiceError extends Data.TaggedError("RepositoryServiceError")<{
	readonly cause: unknown;
	readonly operation: RepositoryReadOperation;
	readonly root_path: string;
}> {}

export class RepositoryService extends Context.Service<
	RepositoryService,
	{
		readonly Inspect: (
			root_path: string,
		) => Effect.Effect<ProjectRepository, RepositoryServiceError>;
		/** Summarizes uncommitted work at the root: the whole working tree against `HEAD`. */
		readonly Diff: (root_path: string) => Effect.Effect<ProjectDiff, RepositoryServiceError>;
	}
>()("Artisan/RepositoryService") {}

const Text = (bytes: Uint8Array) => decoder.decode(bytes).trim();

/**
 * Parses `git config --get-regexp` output into remotes.
 *
 * @param output - Lines of `remote.<name>.url <url>`.
 * @returns One remote per configured name, in Git's own order.
 */
export const ParseConfiguredRemotes = (output: string): ReadonlyArray<RepositoryRemote> => {
	const remotes: Array<RepositoryRemote> = [];
	const seen = new Set<string>();

	for (const line of output.split("\n")) {
		const separator = line.indexOf(" ");
		if (separator === -1) continue;

		const key = line.slice(0, separator);
		const url = line.slice(separator + 1).trim();
		const name = key.replace(/^remote\./, "").replace(/\.url$/, "");

		if (name === "" || url === "" || seen.has(name)) continue;
		if (remotes.length >= maximum_remotes) break;

		seen.add(name);
		const web_url = RepositoryWebUrlFor(url);
		remotes.push({
			host: RepositoryHostFor(url),
			name,
			url,
			...(web_url === undefined ? {} : { web_url }),
		});
	}

	return remotes;
};

/** Prefers `origin`, the convention every host writes on clone, then Git's own order. */
export const DefaultRemoteFor = (remotes: ReadonlyArray<RepositoryRemote>): string | undefined =>
	remotes.find((remote) => remote.name === "origin")?.name ?? remotes[0]?.name;

/**
 * Drops a truncated tail so a bounded read never parses half a record.
 *
 * @param output - Bytes captured from a NUL-delimited Git read.
 * @returns The captured bytes up to and including their last delimiter.
 */
export const WholeRecordsOf = (output: {
	readonly bytes: Uint8Array;
	readonly truncated: boolean;
}): Uint8Array => {
	if (!output.truncated) return output.bytes;

	const last = output.bytes.lastIndexOf(0);

	return last === -1 ? new Uint8Array(0) : output.bytes.subarray(0, last + 1);
};

/**
 * Counts NUL-delimited paths, which is what `-z` list output is.
 *
 * @param bytes - Whole records from `git ls-files -z`.
 * @returns One count per delimited path, ignoring the trailing empty field.
 */
export const CountNulPaths = (bytes: Uint8Array): number => {
	let count = 0;
	let length = 0;

	for (const byte of bytes) {
		if (byte === 0) {
			if (length > 0) count += 1;
			length = 0;
		} else {
			length += 1;
		}
	}

	return length > 0 ? count + 1 : count;
};

/**
 * Reads one non-negative count from Git's own output.
 *
 * @param value - Trimmed stdout of a counting read such as `rev-list --count`.
 * @returns The count, or zero for output that is not one.
 */
export const ParseNonNegativeCount = (value: string): number => {
	if (!/^\d+$/u.test(value)) return 0;

	const count = Number(value);

	return Number.isSafeInteger(count) ? count : 0;
};

/**
 * Reads commit distance from `rev-list --left-right --count <ref>...HEAD`.
 *
 * Left counts commits only the ref has, which is how far HEAD is behind;
 * right counts commits only HEAD has, which is how far it is ahead.
 *
 * @param value - Trimmed stdout holding two tab-separated counts.
 * @returns The distance, or `undefined` for output in any other shape.
 */
export const ParseAheadBehind = (
	value: string,
): { readonly ahead: number; readonly behind: number } | undefined => {
	const parts = value.split(/\s+/u).filter((part) => part.length > 0);

	if (parts.length !== 2 || parts.some((part) => !/^\d+$/u.test(part))) return undefined;

	const behind = Number(parts[0]);
	const ahead = Number(parts[1]);

	return Number.isSafeInteger(behind) && Number.isSafeInteger(ahead)
		? { ahead, behind }
		: undefined;
};

/**
 * Converts Git's own commit timestamp into the protocol's UTC instant.
 *
 * @param value - Trimmed stdout of `log --format=%ct`, in Unix seconds.
 * @returns The instant in UTC, or `undefined` for output that is not one.
 */
export const IsoFromUnixSeconds = (value: string): string | undefined => {
	if (!/^\d+$/u.test(value)) return undefined;

	const milliseconds = Number(value) * 1_000;

	return Number.isSafeInteger(milliseconds) && Math.abs(milliseconds) <= 8.64e15
		? new Date(milliseconds).toISOString()
		: undefined;
};

export const RepositoryServiceLive = Layer.effect(
	RepositoryService,
	Effect.gen(function* () {
		const executor = yield* GitCommandExecutor;

		const Run = (
			root_path: string,
			operation: RepositoryReadOperation,
			args: ReadonlyArray<string>,
			stdout_limit = max_stdout_bytes,
		) =>
			executor
				.Run({
					args,
					cwd: root_path,
					max_stderr_bytes,
					max_stdin_bytes: 0,
					max_stdout_bytes: stdout_limit,
					mode: "read",
				})
				.pipe(
					Effect.mapError(
						(cause) => new RepositoryServiceError({ cause, operation, root_path }),
					),
				);

		const Inspect = (root_path: string) =>
			Effect.gen(function* () {
				const inside = yield* Run(root_path, "discover", [
					"rev-parse",
					"--is-inside-work-tree",
				]);

				/**
				 * A non-zero exit is the ordinary answer for a directory Git does not
				 * track, so it reports state rather than failing the query.
				 */
				if (inside.exit_code !== 0 || Text(inside.stdout.bytes) !== "true") {
					return { state: "not_repository" } as const;
				}

				const [head_ref, head_object, configured] = yield* Effect.all(
					[
						Run(root_path, "branch", ["symbolic-ref", "--quiet", "--short", "HEAD"]),
						Run(root_path, "head", ["rev-parse", "--verify", "--quiet", "HEAD"]),
						Run(root_path, "remotes", [
							"config",
							"--get-regexp",
							String.raw`^remote\..*\.url$`,
						]),
					],
					{ concurrency: 3 },
				);

				const branch_name = head_ref.exit_code === 0 ? Text(head_ref.stdout.bytes) : "";
				const head = head_object.exit_code === 0 ? Text(head_object.stdout.bytes) : "";
				/**
				 * A fresh repository has a symbolic HEAD naming a branch that holds no
				 * commit yet; Git reports the name but resolves no object for it.
				 */
				const branch: GitBranchState =
					branch_name === ""
						? { type: "detached" }
						: head === ""
							? { name: branch_name, type: "unborn" }
							: { name: branch_name, type: "attached" };

				const remotes =
					configured.exit_code === 0
						? ParseConfiguredRemotes(Text(configured.stdout.bytes))
						: [];
				const default_remote = DefaultRemoteFor(remotes);

				return {
					branch,
					...(default_remote === undefined ? {} : { default_remote }),
					...(head === "" || branch.type === "unborn" ? {} : { head }),
					remotes,
					state: "repository",
				} as const;
			});

		/** Zero counts for a read Git declined to answer, so one gap cannot blank the rest. */
		const NoCounts: RepositoryDiffCounts = {
			binary_file_count: 0,
			file_count: 0,
			lines_added: 0,
			lines_deleted: 0,
		};

		/**
		 * Reads counts from one numstat result, reporting whether they are whole.
		 *
		 * A read that overflowed its byte cap, or a tail the parser rejects, must
		 * not be reported as zero: "no changes" is the most misleading answer
		 * available for a repository that in fact changed thousands of files. The
		 * partial figure travels with `truncated` so the reader can say so.
		 */
		const NumstatCounts = (
			result: GitCommandResult,
		): Effect.Effect<{
			readonly counts: RepositoryDiffCounts;
			readonly truncated: boolean;
		}> => {
			const limited = result.termination === "output_limit" || result.stdout.truncated;

			if (result.exit_code !== 0 && !limited) {
				return Effect.succeed({ counts: NoCounts, truncated: false });
			}

			return ParseGitNumstat(WholeRecordsOf(result.stdout)).pipe(
				Effect.map((stats) => ({
					counts: {
						binary_file_count: stats.binary_files,
						file_count: stats.files,
						lines_added: stats.additions,
						lines_deleted: stats.deletions,
					},
					truncated: limited,
				})),
				/**
				 * Trimming to the last delimiter can still split a rename record,
				 * which carries its paths in the two fields after the counts. An
				 * unparsable tail is a truncation, not a repository without changes.
				 */
				Effect.catch(() => Effect.succeed({ counts: NoCounts, truncated: true })),
			);
		};

		const Numstat = (root_path: string, scope: ReadonlyArray<string>) =>
			Run(
				root_path,
				"diff",
				[
					"-c",
					"core.quotePath=true",
					"diff",
					"--no-ext-diff",
					"--no-textconv",
					"--numstat",
					"-z",
					...scope,
				],
				max_diff_stdout_bytes,
			);

		/**
		 * Compares the checked-out branch against one ref.
		 *
		 * Three-dot rather than two: `ref...` measures the merge base against HEAD,
		 * so a branch that is merely behind its upstream does not report the other
		 * side's commits as its own reversed changes.
		 */
		const CompareRef = (
			root_path: string,
			kind: RepositoryBranchComparison["kind"],
			ref: string,
		): Effect.Effect<RepositoryBranchComparison | undefined, RepositoryServiceError> =>
			Effect.all(
				[
					Run(root_path, "ahead_behind", [
						"rev-list",
						"--left-right",
						"--count",
						"--end-of-options",
						`${ref}...HEAD`,
					]),
					Numstat(root_path, ["--end-of-options", `${ref}...`]),
				],
				{ concurrency: 2 },
			).pipe(
				Effect.flatMap(([range, numstat]) =>
					NumstatCounts(numstat).pipe(
						Effect.map((measured) => {
							const distance =
								range.exit_code === 0
									? ParseAheadBehind(Text(range.stdout.bytes))
									: undefined;

							/**
							 * `origin/HEAD` can name a branch that no longer exists — an
							 * upstream default-branch rename leaves it dangling, and
							 * `symbolic-ref` still resolves it happily. Reporting that as
							 * `0 ahead · 0 behind` would invent a baseline, so an
							 * unreadable range yields no comparison at all.
							 */
							if (distance === undefined) return undefined;

							return {
								ahead: distance.ahead,
								behind: distance.behind,
								counts: measured.counts,
								kind,
								ref,
							};
						}),
					),
				),
			);

		/**
		 * Summarizes the working tree against `HEAD`, then against every branch ref
		 * that resolves.
		 *
		 * `HEAD` leads because it is the one baseline that always answers, so the
		 * headline reading never silently becomes "nothing" for want of a remote or
		 * a fetch. The branch comparisons are additive: each is reported only when
		 * its ref exists, rather than guessing at a default branch name.
		 *
		 * Untracked files are counted but not measured. `git diff` cannot see them,
		 * and the alternatives — writing intent-to-add entries into the user's
		 * index, or reading every new file to count its lines — either mutate
		 * working state for a read or turn a badge into a filesystem walk. The
		 * caller gets the tally and reports it as its own fact.
		 */
		const Diff = (root_path: string) =>
			Effect.gen(function* () {
				const inside = yield* Run(root_path, "discover", [
					"rev-parse",
					"--is-inside-work-tree",
				]);

				if (inside.exit_code !== 0 || Text(inside.stdout.bytes) !== "true") {
					return { state: "not_repository" } as const;
				}

				const head_object = yield* Run(root_path, "head", [
					"rev-parse",
					"--verify",
					"--quiet",
					"HEAD",
				]);
				const has_head =
					head_object.exit_code === 0 && Text(head_object.stdout.bytes) !== "";
				/** Before the first commit there is no HEAD, so the index is the whole story. */
				const working_scope = has_head ? ["HEAD"] : ["--cached"];
				const staged_scope = has_head ? ["--cached", "HEAD"] : ["--cached"];

				const [
					working,
					staged,
					unstaged,
					untracked,
					stash,
					committed,
					upstream,
					remote_head,
				] = yield* Effect.all(
					[
						Numstat(root_path, working_scope),
						Numstat(root_path, staged_scope),
						Numstat(root_path, []),
						Run(
							root_path,
							"untracked",
							["ls-files", "--others", "--exclude-standard", "-z"],
							max_diff_stdout_bytes,
						),
						/**
						 * `--walk-reflogs` counts stash entries. Without it `rev-list` walks
						 * each entry's ancestry and reports the whole reachable history.
						 */
						Run(root_path, "stash", [
							"rev-list",
							"--walk-reflogs",
							"--count",
							"refs/stash",
						]),
						/**
						 * Unix seconds rather than `%cI`: Git prints the committer's own
						 * offset, and the protocol carries UTC only.
						 */
						Run(root_path, "head", ["log", "-1", "--format=%ct"]),
						Run(root_path, "upstream", [
							"rev-parse",
							"--abbrev-ref",
							"--symbolic-full-name",
							"@{upstream}",
						]),
						Run(root_path, "default_branch", [
							"symbolic-ref",
							"--quiet",
							"--short",
							"refs/remotes/origin/HEAD",
						]),
					],
					{ concurrency: 4 },
				);

				const upstream_ref = upstream.exit_code === 0 ? Text(upstream.stdout.bytes) : "";
				const default_ref =
					remote_head.exit_code === 0 ? Text(remote_head.stdout.bytes) : "";
				/** A branch that tracks the default branch has one baseline, not two identical ones. */
				const comparable: ReadonlyArray<{
					readonly kind: "default_branch" | "upstream";
					readonly ref: string;
				}> = has_head
					? [
							...(upstream_ref === ""
								? []
								: [{ kind: "upstream" as const, ref: upstream_ref }]),
							...(default_ref === "" || default_ref === upstream_ref
								? []
								: [{ kind: "default_branch" as const, ref: default_ref }]),
						]
					: [];

				const [working_counts, staged_counts, unstaged_counts] = yield* Effect.all(
					[NumstatCounts(working), NumstatCounts(staged), NumstatCounts(unstaged)],
					{ concurrency: 3 },
				);
				const compared = yield* Effect.forEach(
					comparable,
					(target) => CompareRef(root_path, target.kind, target.ref),
					{ concurrency: 2 },
				);
				/**
				 * A branch level with its baseline is not news. Reporting it would hold
				 * the lip open showing `+0 −0` for the most common repository state.
				 */
				const comparisons = compared.filter(
					(comparison): comparison is RepositoryBranchComparison =>
						comparison !== undefined &&
						(comparison.ahead > 0 ||
							comparison.behind > 0 ||
							comparison.counts.file_count > 0),
				);

				const head_committed_at =
					committed.exit_code === 0
						? IsoFromUnixSeconds(Text(committed.stdout.bytes))
						: undefined;

				const untracked_limited =
					untracked.termination === "output_limit" || untracked.stdout.truncated;

				return {
					comparisons,
					...(head_committed_at === undefined ? {} : { head_committed_at }),
					staged: staged_counts.counts,
					state: "repository",
					stash_count:
						stash.exit_code === 0 ? ParseNonNegativeCount(Text(stash.stdout.bytes)) : 0,
					/** Any partial read taints the whole reading, so one flag covers them all. */
					truncated:
						working_counts.truncated ||
						staged_counts.truncated ||
						unstaged_counts.truncated ||
						untracked_limited,
					unstaged: unstaged_counts.counts,
					untracked_file_count:
						untracked.exit_code === 0 || untracked_limited
							? CountNulPaths(WholeRecordsOf(untracked.stdout))
							: 0,
					working: working_counts.counts,
				} as const;
			});

		return { Diff, Inspect };
	}),
);
