import { Buffer } from "node:buffer";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Git } from "../../modules/backend/src/git/git";
import {
	GitCommandExecutor,
	GitCommandExecutorError,
	type GitCommandInput,
	type GitCommandResult,
} from "../../modules/backend/src/git/git-command-executor";
import { make_git_layer, make_node_git_layer } from "../../modules/backend/src/git/node-git";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan git repository "));

	roots.push(root);

	return root;
}

async function make_git(root: string) {
	return Effect.runPromise(
		Effect.service(Git).pipe(Effect.provide(make_node_git_layer({ cwd: root }))),
	);
}

function process_result(stdout: string, input: GitCommandInput): GitCommandResult {
	const all_stdout = new TextEncoder().encode(stdout);
	const limit = input.max_stdout_bytes;
	const retained_stdout = all_stdout.slice(0, limit);

	return {
		exit_code: 0,
		stderr: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
		stdout: {
			bytes: retained_stdout,
			total_bytes: all_stdout.byteLength,
			truncated: retained_stdout.byteLength < all_stdout.byteLength,
		},
	};
}

async function make_injected_git(
	status_output: string,
	patch_output = "",
	max_patch_bytes = 1_000_000,
) {
	const root = await make_root();
	const worktree_path = root.replaceAll("\\", "/");
	const executor_test = Layer.succeed(GitCommandExecutor, {
		Run: (input) => {
			const stdout = input.args.includes("--is-inside-work-tree")
				? "true\n"
				: input.args.includes("status")
					? status_output
					: input.args.includes("worktree")
						? `worktree ${worktree_path}\0HEAD ${"1".repeat(40)}\0branch refs/heads/main\0\0`
						: input.args.includes("--numstat")
							? ""
							: input.args.includes("--binary")
								? patch_output
								: "";

			return Effect.succeed(process_result(stdout, input));
		},
	});
	const git_test = make_git_layer({ cwd: root, max_patch_bytes }).pipe(
		Layer.provide(executor_test),
	);

	return Effect.runPromise(Effect.service(Git).pipe(Effect.provide(git_test)));
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("Git", () => {
	it("reports staged, unstaged, untracked, and renamed paths explicitly", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await fs.writeFile(join(root, "staged.txt"), "base\n");
		await fs.writeFile(join(root, "unstaged.txt"), "base\n");
		await fs.writeFile(join(root, "old path.txt"), "base\n");
		await run_process(root, ["add", "."]);
		await run_process(root, ["commit", "-qm", "initial"]);

		await fs.writeFile(join(root, "staged.txt"), "staged\n");
		await run_process(root, ["add", "staged.txt"]);
		await fs.writeFile(join(root, "unstaged.txt"), "unstaged\n");
		await run_process(root, ["mv", "old path.txt", "renamed path.txt"]);
		await fs.writeFile(join(root, "untracked path.txt"), "untracked\n");

		const git = await make_git(root);
		const status = await Effect.runPromise(git.Status);
		const by_path = new Map(status.map((summary) => [summary.path, summary]));

		expect(status).toHaveLength(4);
		expect(by_path.get("staged.txt")).toMatchObject({
			conflicted: false,
			staged: true,
			untracked: false,
			unstaged: false,
		});
		expect(by_path.get("unstaged.txt")).toMatchObject({
			conflicted: false,
			staged: false,
			untracked: false,
			unstaged: true,
		});
		expect(by_path.get("untracked path.txt")).toMatchObject({
			conflicted: false,
			staged: false,
			untracked: true,
			unstaged: false,
		});
		expect(by_path.get("renamed path.txt")).toMatchObject({
			conflicted: false,
			original_path: "old path.txt",
			staged: true,
			status: "R ",
			untracked: false,
			unstaged: false,
		});
	});

	it("consumes copy paths and marks conflicts without inventing staged state", async () => {
		const oid = "1".repeat(40);
		const git = await make_injected_git(
			[
				`# branch.oid ${oid}`,
				"# branch.head main",
				`2 C. N... 100644 100644 100644 ${oid} ${oid} C100 copied name.txt`,
				"source name.txt",
				`u UU N... 100644 100644 100644 100644 ${oid} ${oid} ${oid} conflict.txt`,
				"? loose file.txt",
				"",
			].join("\0"),
		);
		const status = await Effect.runPromise(git.Status);

		expect(status).toEqual([
			{
				conflicted: false,
				original_path: "source name.txt",
				path: "copied name.txt",
				staged: true,
				status: "C ",
				untracked: false,
				unstaged: false,
			},
			{
				conflicted: true,
				path: "conflict.txt",
				staged: false,
				status: "UU",
				untracked: false,
				unstaged: false,
			},
			{
				conflicted: false,
				path: "loose file.txt",
				staged: false,
				status: "??",
				untracked: true,
				unstaged: false,
			},
		]);
	});

	it("bounds patches by complete UTF-8 bytes and reports truncation", async () => {
		const oid = "1".repeat(40);
		const git = await make_injected_git(`# branch.oid ${oid}\0# branch.head main\0`, "åb", 8);

		const two_bytes = await Effect.runPromise(git.DiffPatch(2));
		const one_byte = await Effect.runPromise(git.DiffPatch(1));

		expect(two_bytes).toEqual({ bytes: 2, patch: "å", truncated: true });
		expect(Buffer.byteLength(two_bytes.patch, "utf8")).toBe(2);
		expect(one_byte).toEqual({ bytes: 0, patch: "", truncated: true });
	});

	it("discovers an unborn branch without requiring HEAD", async () => {
		const root = await make_root();

		await initialize_repository(root);

		const git = await make_git(root);
		const repository = await Effect.runPromise(git.Discover);

		expect(repository.root.replaceAll("/", "\\")).toBe(root);
		expect(repository.branch.length).toBeGreaterThan(0);
		expect(Option.isNone(repository.head)).toBe(true);
	});

	it("returns the requested Git operation when an injected process fails", async () => {
		const root = await make_root();
		const process_error = new GitCommandExecutorError({
			cause: new Error("spawn failed"),
			operation: "process",
		});
		const executor_test = Layer.succeed(GitCommandExecutor, {
			Run: () => Effect.fail(process_error),
		});
		const git_test = make_git_layer({ cwd: root }).pipe(Layer.provide(executor_test));
		const git = await Effect.runPromise(Effect.service(Git).pipe(Effect.provide(git_test)));

		const error = await Effect.runPromise(git.Status.pipe(Effect.flip));

		expect(error.operation).toBe("status");
		expect(JSON.stringify(error.cause)).toContain("GitReadError");
	});

	it("returns a typed discover failure outside a repository", async () => {
		const root = await make_root();
		const git = await make_git(root);

		const error = await Effect.runPromise(git.Discover.pipe(Effect.flip));

		expect(error._tag).toBe("GitError");
		expect(error.operation).toBe("discover");
	});
});

async function initialize_repository(root: string) {
	await run_process(root, ["init", "-q"]);
	await run_process(root, ["config", "user.email", "test@example.com"]);
	await run_process(root, ["config", "user.name", "Test User"]);
}

async function run_process(cwd: string, args: ReadonlyArray<string>) {
	const { spawn } = await import("node:child_process");

	await new Promise<void>((resolve, reject) => {
		const child = spawn("git", [...args], { cwd, shell: false, stdio: "ignore" });

		child.on("error", reject);
		child.on("close", (code) =>
			code === 0 ? resolve() : reject(new Error(`git exited ${code}`)),
		);
	});
}
