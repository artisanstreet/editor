import { Data, Effect, Schema } from "effect";

import {
	GitDiffStats,
	GitParsedStatus,
	GitWorktree,
	type GitFileStatus,
	type GitHead,
	type GitUpstream,
} from "./git-model";

export type GitParseFormat = "numstat" | "status_v2" | "worktree_list";

/** Reports malformed, truncated, or non-UTF-8 machine-readable Git output. */
export class GitParseError extends Data.TaggedError("GitParseError")<{
	readonly cause: unknown;
	readonly format: GitParseFormat;
}> {}

const decode_utf8 = (bytes: Uint8Array) => new TextDecoder("utf-8", { fatal: true }).decode(bytes);

const parse_error = (format: GitParseFormat, cause: unknown) =>
	new GitParseError({ cause, format });

const decode_result = <A>(
	format: GitParseFormat,
	schema: Schema.Codec<A, unknown, never, unknown>,
	value: unknown,
) =>
	Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError((cause) => parse_error(format, cause)),
	);

function split_fixed_fields(record: string, field_count: number) {
	const fields: Array<string> = [];
	let offset = 0;

	for (let index = 0; index < field_count; index += 1) {
		const separator = record.indexOf(" ", offset);

		if (separator < 0) {
			throw new Error("Git record is missing a fixed field");
		}

		const field = record.slice(offset, separator);

		if (field.length === 0) {
			throw new Error("Git record contains an empty fixed field");
		}

		fields.push(field);
		offset = separator + 1;
	}

	const path = record.slice(offset);

	if (path.length === 0) {
		throw new Error("Git record is missing its path");
	}

	return { fields, path };
}

function status_flags(status: string, kind: GitFileStatus["kind"]) {
	if (!/^[.MADRCUT? !]{2}$/u.test(status)) {
		throw new Error("Git record contains an unsupported XY status");
	}

	const conflicted = kind === "unmerged";
	const untracked = kind === "untracked";
	const ignored = kind === "ignored";

	return {
		conflicted,
		index_status: status[0]!,
		staged: !conflicted && !untracked && !ignored && status[0] !== ".",
		status,
		untracked,
		unstaged: !conflicted && !untracked && !ignored && status[1] !== ".",
		worktree_status: status[1]!,
	};
}

function make_tracked_file(
	record: string,
	kind: "ordinary" | "renamed" | "unmerged",
	original_path?: string,
): GitFileStatus {
	const fixed_count = kind === "ordinary" ? 8 : kind === "renamed" ? 9 : 10;
	const { fields, path } = split_fixed_fields(record, fixed_count);
	const status = fields[1];
	const submodule = fields[2];

	if (status === undefined || status.length !== 2 || submodule === undefined) {
		throw new Error("Git record is missing status metadata");
	}

	return {
		...status_flags(status, kind),
		kind,
		...(original_path === undefined ? {} : { original_path }),
		path,
		...(submodule === "N..." ? {} : { submodule }),
	};
}

function set_once(current: string | undefined, value: string, field: string): string {
	if (current !== undefined) {
		throw new Error(`Git status repeated ${field}`);
	}

	return value;
}

function parse_status_unsafe(bytes: Uint8Array) {
	const records = decode_utf8(bytes).split("\0");
	const files: Array<GitFileStatus> = [];
	let branch_ab: string | undefined;
	let branch_head: string | undefined;
	let branch_oid: string | undefined;
	let branch_upstream: string | undefined;

	for (let index = 0; index < records.length; index += 1) {
		const record = records[index]!;

		if (record.length === 0) {
			continue;
		}

		if (record.startsWith("# branch.oid ")) {
			branch_oid = set_once(branch_oid, record.slice("# branch.oid ".length), "branch.oid");
			continue;
		}

		if (record.startsWith("# branch.head ")) {
			branch_head = set_once(
				branch_head,
				record.slice("# branch.head ".length),
				"branch.head",
			);
			continue;
		}

		if (record.startsWith("# branch.upstream ")) {
			branch_upstream = set_once(
				branch_upstream,
				record.slice("# branch.upstream ".length),
				"branch.upstream",
			);
			continue;
		}

		if (record.startsWith("# branch.ab ")) {
			branch_ab = set_once(branch_ab, record.slice("# branch.ab ".length), "branch.ab");
			continue;
		}

		if (record.startsWith("# ")) {
			continue;
		}

		if (record.startsWith("1 ")) {
			files.push(make_tracked_file(record, "ordinary"));
			continue;
		}

		if (record.startsWith("2 ")) {
			const original_path = records[index + 1];

			if (original_path === undefined || original_path.length === 0) {
				throw new Error("Git rename or copy record is missing its original path");
			}

			files.push(make_tracked_file(record, "renamed", original_path));
			index += 1;
			continue;
		}

		if (record.startsWith("u ")) {
			files.push(make_tracked_file(record, "unmerged"));
			continue;
		}

		if (record.startsWith("? ") || record.startsWith("! ")) {
			const kind = record[0] === "?" ? "untracked" : "ignored";
			const status = kind === "untracked" ? "??" : "!!";

			files.push({
				...status_flags(status, kind),
				kind,
				path: record.slice(2),
			});
			continue;
		}

		throw new Error("Git status contains an unknown porcelain-v2 record");
	}

	if (!branch_oid || !branch_head) {
		throw new Error("Git status is missing branch identity headers");
	}

	let head: GitHead;

	if (branch_oid === "(initial)") {
		if (branch_head === "(detached)") {
			throw new Error("An unborn Git repository cannot have a detached HEAD");
		}

		head = { _tag: "unborn", branch: branch_head };
	} else if (branch_head === "(detached)") {
		head = { _tag: "detached", oid: branch_oid };
	} else {
		head = { _tag: "attached", branch: branch_head, oid: branch_oid };
	}

	let upstream: GitUpstream = { _tag: "none" };

	if (branch_upstream !== undefined || branch_ab !== undefined) {
		if (branch_upstream === undefined || branch_ab === undefined) {
			throw new Error("Git status contains incomplete upstream metadata");
		}

		const match = /^\+(\d+) -(\d+)$/u.exec(branch_ab);

		if (!match) {
			throw new Error("Git status contains malformed ahead/behind metadata");
		}

		upstream = {
			_tag: "tracked",
			ahead: Number(match[1]),
			behind: Number(match[2]),
			ref: branch_upstream,
		};
	}

	return { files, head, upstream };
}

/** Parses bounded `git status --porcelain=v2 --branch -z` bytes. */
export const ParseGitStatus = (bytes: Uint8Array) =>
	Effect.try({
		try: () => parse_status_unsafe(bytes),
		catch: (cause) => parse_error("status_v2", cause),
	}).pipe(
		Effect.flatMap((status) =>
			decode_result<typeof GitParsedStatus.Type>("status_v2", GitParsedStatus, status),
		),
	);

interface MutableWorktree {
	bare: boolean;
	branch?: string;
	current: boolean;
	detached: boolean;
	head?: string;
	locked_reason?: string;
	path: string;
	prunable_reason?: string;
}

function finalize_worktree(worktree: MutableWorktree | undefined) {
	if (worktree === undefined) {
		return undefined;
	}

	if (!worktree.bare && worktree.head === undefined) {
		throw new Error("A non-bare Git worktree is missing HEAD");
	}

	if (worktree.detached && worktree.branch !== undefined) {
		throw new Error("A Git worktree cannot be both detached and attached");
	}

	return worktree;
}

function parse_worktrees_unsafe(bytes: Uint8Array) {
	const fields = decode_utf8(bytes).split("\0");
	const worktrees: Array<MutableWorktree> = [];
	let current: MutableWorktree | undefined;

	for (const field of fields) {
		if (field.length === 0) {
			const completed = finalize_worktree(current);

			if (completed !== undefined) {
				worktrees.push(completed);
				current = undefined;
			}

			continue;
		}

		if (field.startsWith("worktree ")) {
			if (current !== undefined) {
				throw new Error("Git worktree records are not separated");
			}

			current = {
				bare: false,
				current: false,
				detached: false,
				path: field.slice("worktree ".length),
			};
			continue;
		}

		if (current === undefined) {
			throw new Error("Git worktree metadata appears before its path");
		}

		if (field.startsWith("HEAD ")) {
			if (current.head !== undefined) {
				throw new Error("Git worktree repeated HEAD");
			}

			current.head = field.slice("HEAD ".length);
		} else if (field.startsWith("branch ")) {
			if (current.branch !== undefined) {
				throw new Error("Git worktree repeated branch");
			}

			current.branch = field.slice("branch ".length);
		} else if (field === "bare") {
			current.bare = true;
		} else if (field === "detached") {
			current.detached = true;
		} else if (field === "locked" || field.startsWith("locked ")) {
			current.locked_reason = field === "locked" ? "" : field.slice("locked ".length);
		} else if (field === "prunable" || field.startsWith("prunable ")) {
			current.prunable_reason = field === "prunable" ? "" : field.slice("prunable ".length);
		} else {
			throw new Error("Git worktree contains unknown porcelain metadata");
		}
	}

	if (current !== undefined) {
		throw new Error("Git worktree output is missing its record terminator");
	}

	return worktrees;
}

/** Parses bounded `git worktree list --porcelain -z` bytes. */
export const ParseGitWorktrees = (bytes: Uint8Array) =>
	Effect.try({
		try: () => parse_worktrees_unsafe(bytes),
		catch: (cause) => parse_error("worktree_list", cause),
	}).pipe(
		Effect.flatMap((worktrees) =>
			decode_result<ReadonlyArray<typeof GitWorktree.Type>>(
				"worktree_list",
				Schema.Array(GitWorktree),
				worktrees,
			),
		),
	);

function parse_count(value: string) {
	if (!/^\d+$/u.test(value)) {
		throw new Error("Git numstat contains a non-numeric line count");
	}

	const count = Number(value);

	if (!Number.isSafeInteger(count)) {
		throw new Error("Git numstat line count exceeds the safe integer range");
	}

	return count;
}

function add_safe(left: number, right: number) {
	const total = left + right;

	if (!Number.isSafeInteger(total)) {
		throw new Error("Git numstat aggregate exceeds the safe integer range");
	}

	return total;
}

function parse_numstat_unsafe(bytes: Uint8Array) {
	const fields = decode_utf8(bytes).split("\0");
	let additions = 0;
	let binary_files = 0;
	let deletions = 0;
	let files = 0;

	for (let index = 0; index < fields.length; index += 1) {
		const field = fields[index]!;

		if (field.length === 0) {
			continue;
		}

		const first_separator = field.indexOf("\t");
		const second_separator = field.indexOf("\t", first_separator + 1);

		if (first_separator < 0 || second_separator < 0) {
			throw new Error("Git numstat record is missing tab-separated counts");
		}

		const added = field.slice(0, first_separator);
		const deleted = field.slice(first_separator + 1, second_separator);
		const path = field.slice(second_separator + 1);

		if (path.length === 0) {
			const old_path = fields[index + 1];
			const new_path = fields[index + 2];

			if (!old_path || !new_path) {
				throw new Error("Git rename numstat record is missing one of its paths");
			}

			index += 2;
		}

		if (added === "-" || deleted === "-") {
			if (added !== "-" || deleted !== "-") {
				throw new Error("Git binary numstat record has inconsistent markers");
			}

			binary_files = add_safe(binary_files, 1);
		} else {
			additions = add_safe(additions, parse_count(added));
			deletions = add_safe(deletions, parse_count(deleted));
		}

		files = add_safe(files, 1);
	}

	return { additions, binary_files, deletions, files };
}

/** Parses bounded `git diff --numstat -z` output without locale-sensitive text. */
export const ParseGitNumstat = (bytes: Uint8Array) =>
	Effect.try({
		try: () => parse_numstat_unsafe(bytes),
		catch: (cause) => parse_error("numstat", cause),
	}).pipe(
		Effect.flatMap((stats) =>
			decode_result<typeof GitDiffStats.Type>("numstat", GitDiffStats, stats),
		),
	);
