import { Effect, Layer, ManagedRuntime, Schema } from "effect";
import { describe, expect, it } from "vitest";

import { ProjectDiff } from "@artisan/protocol";

import { GitCommandExecutor, type GitCommandResult } from "../../modules/backend/src/git/executor";
import {
	RepositoryService,
	RepositoryServiceLive,
} from "../../modules/backend/src/git/repository-service";

const encoder = new TextEncoder();

const output = (value: string, truncated = false) => ({
	bytes: encoder.encode(value),
	total_bytes: encoder.encode(value).byteLength,
	truncated,
});

const ok = (stdout: string): GitCommandResult => ({
	exit_code: 0,
	stderr: output(""),
	stdout: output(stdout),
});

const failed: GitCommandResult = { exit_code: 128, stderr: output("fatal"), stdout: output("") };

/** Names one stubbed read by the distinguishing token in its arguments. */
const read_name = (args: ReadonlyArray<string>): string => {
	if (args.includes("--is-inside-work-tree")) return "inside";
	if (args.includes("@{upstream}")) return "upstream";
	if (args.includes("refs/remotes/origin/HEAD")) return "default_branch";
	if (args.includes("refs/stash")) return "stash";
	if (args[0] === "log") return "committed";
	if (args.includes("--verify")) return "head";
	if (args.includes("--left-right")) return "ahead_behind";
	if (args.includes("--others")) return "untracked";
	if (args.includes("--numstat")) {
		const scope = args.slice(args.indexOf("-z") + 1);
		if (scope.some((part) => part.includes("..."))) return "compare";
		if (scope[0] === "--cached" && scope[1] === "HEAD") return "staged";
		if (scope[0] === "--cached") return "cached";
		if (scope[0] === "HEAD") return "working";
		return "unstaged";
	}
	return args.join(" ");
};

/** Reads the diff against a scripted Git, so orchestration is exercised without a repository. */
const read_diff = (
	reads: Readonly<Record<string, GitCommandResult>>,
	root_path = "C:/repo",
): Promise<unknown> => {
	const executor = Layer.succeed(GitCommandExecutor, {
		Run: (input) => Effect.succeed(reads[read_name(input.args)] ?? failed),
	});
	const runtime = ManagedRuntime.make(RepositoryServiceLive.pipe(Layer.provide(executor)));

	return runtime
		.runPromise(RepositoryService.pipe(Effect.flatMap((service) => service.Diff(root_path))))
		.finally(() => void runtime.dispose());
};

const clean_repository = {
	committed: ok("1785313812"),
	default_branch: failed,
	head: ok("0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c"),
	inside: ok("true"),
	staged: ok(""),
	stash: failed,
	unstaged: ok(""),
	untracked: ok(""),
	upstream: failed,
	working: ok(""),
} satisfies Readonly<Record<string, GitCommandResult>>;

describe("project diff reads", () => {
	/**
	 * The regression that matters most: the backend never validates outbound
	 * payloads, so a snapshot that fails this decode reaches a client that fails
	 * it strictly and drops the whole transport session.
	 */
	it("produces a snapshot the protocol accepts", async () => {
		const snapshot = await read_diff({
			...clean_repository,
			untracked: ok("src/new.ts\0"),
			working: ok("12\t3\tsrc/a.ts\0"),
		});

		expect(() => Schema.decodeUnknownSync(ProjectDiff)(snapshot)).not.toThrow();
		expect(snapshot).toMatchObject({
			head_committed_at: "2026-07-29T08:30:12.000Z",
			state: "repository",
			truncated: false,
			untracked_file_count: 1,
			working: { file_count: 1, lines_added: 12, lines_deleted: 3 },
		});
	});

	it("reads a directory Git does not track as no repository", async () => {
		expect(await read_diff({ inside: ok("false") })).toEqual({ state: "not_repository" });
	});

	/** Before the first commit there is no HEAD to diff against, so the index is the whole story. */
	it("measures the index alone on an unborn branch", async () => {
		const snapshot = await read_diff({
			...clean_repository,
			cached: ok("4\t0\tsrc/first.ts\0"),
			committed: failed,
			head: failed,
			working: failed,
		});

		expect(snapshot).toMatchObject({
			comparisons: [],
			working: { file_count: 1, lines_added: 4 },
		});
		expect(snapshot).not.toHaveProperty("head_committed_at");
	});

	it("counts stash entries only from a successful read", async () => {
		expect(await read_diff({ ...clean_repository, stash: ok("2") })).toMatchObject({
			stash_count: 2,
		});
		expect(await read_diff(clean_repository)).toMatchObject({ stash_count: 0 });
	});

	it("reports the upstream comparison when the branch has diverged", async () => {
		const snapshot = (await read_diff({
			...clean_repository,
			ahead_behind: ok("1\t3"),
			compare: ok("40\t9\tsrc/a.ts\0"),
			upstream: ok("origin/feature"),
		})) as { readonly comparisons: ReadonlyArray<Record<string, unknown>> };

		expect(snapshot.comparisons).toEqual([
			{
				ahead: 3,
				behind: 1,
				counts: {
					binary_file_count: 0,
					file_count: 1,
					lines_added: 40,
					lines_deleted: 9,
				},
				kind: "upstream",
				ref: "origin/feature",
			},
		]);
	});

	/** A branch level with its baseline is not news; reporting it would hold the lip open. */
	it("omits a comparison against a ref it is level with", async () => {
		expect(
			await read_diff({
				...clean_repository,
				ahead_behind: ok("0\t0"),
				compare: ok(""),
				upstream: ok("origin/master"),
			}),
		).toMatchObject({ comparisons: [] });
	});

	/**
	 * `symbolic-ref` resolves `origin/HEAD` even when the branch it names is gone,
	 * which an upstream default-branch rename leaves behind. Reporting that as
	 * `0 ahead · 0 behind` would invent a baseline.
	 */
	it("omits a dangling default branch rather than inventing zeroes", async () => {
		expect(
			await read_diff({
				...clean_repository,
				ahead_behind: failed,
				compare: failed,
				default_branch: ok("origin/main"),
			}),
		).toMatchObject({ comparisons: [] });
	});

	it("reports one comparison when the upstream is also the default branch", async () => {
		const snapshot = (await read_diff({
			...clean_repository,
			ahead_behind: ok("0\t2"),
			compare: ok("5\t1\tsrc/a.ts\0"),
			default_branch: ok("origin/master"),
			upstream: ok("origin/master"),
		})) as { readonly comparisons: ReadonlyArray<{ readonly kind: string }> };

		expect(snapshot.comparisons.map((comparison) => comparison.kind)).toEqual(["upstream"]);
	});

	/** Zero counts for a read that overflowed would turn "thousands changed" into "clean". */
	it("marks a truncated read rather than reporting it as clean", async () => {
		const overflowed: GitCommandResult = {
			exit_code: -1,
			stderr: output(""),
			stdout: output("12\t3\tsrc/a.ts\0", true),
			termination: "output_limit",
		};

		expect(await read_diff({ ...clean_repository, working: overflowed })).toMatchObject({
			truncated: true,
			working: { file_count: 1, lines_added: 12 },
		});
	});

	/** Trimming to the last delimiter can split a rename record, whose paths follow the counts. */
	it("marks a read whose tail cannot be parsed rather than failing the whole diff", async () => {
		const split_rename: GitCommandResult = {
			exit_code: 0,
			stderr: output(""),
			stdout: output("3\t4\t\0src/old.ts\0", true),
			termination: "output_limit",
		};

		expect(await read_diff({ ...clean_repository, working: split_rename })).toMatchObject({
			truncated: true,
			working: { file_count: 0 },
		});
	});
});
