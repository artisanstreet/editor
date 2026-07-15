import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GitFetch } from "../../modules/backend/src/git/git-fetch";
import {
	make_git_fetch_layer,
	make_node_git_fetch_layer,
} from "../../modules/backend/src/git/node-git-mutation";
import { make_node_process_runner_layer } from "../../modules/backend/src/git/node-process-runner";
import {
	ProcessRunner,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-git-fetch-"));

	roots.push(root);

	return root;
}

async function run_process(cwd: string, command: string, args: ReadonlyArray<string>) {
	return new Promise<{ readonly stderr: string; readonly stdout: string }>((resolve, reject) => {
		const child = spawn(command, [...args], {
			cwd,
			env: process.env,
			shell: false,
			stdio: ["ignore", "pipe", "pipe"],
			windowsHide: true,
		});
		const stdout: Array<Buffer> = [];
		const stderr: Array<Buffer> = [];

		child.stdout.on("data", (chunk: Buffer) => stdout.push(chunk));
		child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
		child.on("error", reject);
		child.on("close", (code) => {
			const output = {
				stderr: Buffer.concat(stderr).toString("utf8"),
				stdout: Buffer.concat(stdout).toString("utf8"),
			};

			if (code === 0) {
				resolve(output);

				return;
			}

			reject(new Error(`${command} ${args.join(" ")} exited ${code}: ${output.stderr}`));
		});
	});
}

async function find_git_executable() {
	const names = process.platform === "win32" ? ["git.exe", "git.com"] : ["git"];
	const candidates = (process.env.PATH ?? "")
		.split(delimiter)
		.map((entry) => entry.replace(/^"|"$/gu, ""))
		.filter((entry) => entry.length > 0)
		.flatMap((entry) => names.map((name) => join(entry, name)));

	for (const candidate of candidates) {
		try {
			const canonical = await fs.realpath(candidate);
			const stat = await fs.stat(canonical);

			if (stat.isFile()) {
				return canonical;
			}
		} catch {}
	}

	throw new Error("Git executable is unavailable");
}

async function run_git(cwd: string, args: ReadonlyArray<string>) {
	const git = await find_git_executable();

	return run_process(cwd, git, args);
}

async function initialize_repository(root: string) {
	await run_git(root, ["init", "--quiet", "--initial-branch=main"]);
	await run_git(root, ["config", "core.autocrlf", "false"]);
	await run_git(root, ["config", "user.email", "tests@example.com"]);
	await run_git(root, ["config", "user.name", "Artisan Tests"]);
	await fs.writeFile(join(root, "tracked.txt"), "initial\n");
	await run_git(root, ["add", "tracked.txt"]);
	await run_git(root, ["commit", "--quiet", "--message", "initial"]);
}

async function commit_file(root: string, path: string, content: string, message: string) {
	await fs.writeFile(join(root, path), content);
	await run_git(root, ["add", "--", path]);
	await run_git(root, ["commit", "--quiet", "--message", message]);
}

async function make_remote_fixture() {
	const root = await make_root();
	const remote = join(root, "remote.git");
	const source = join(root, "source");
	const workspace = join(root, "workspace");

	await fs.mkdir(remote);
	await run_git(remote, ["init", "--bare", "--quiet", "--initial-branch=main"]);
	await fs.mkdir(source);
	await initialize_repository(source);
	await run_git(source, ["remote", "add", "origin", remote]);
	await run_git(source, ["push", "--quiet", "--set-upstream", "origin", "main"]);
	await run_git(source, ["switch", "--quiet", "--create", "feature"]);
	await commit_file(source, "feature.txt", "feature\n", "feature");
	await run_git(source, ["push", "--quiet", "--set-upstream", "origin", "feature"]);
	await run_git(source, ["switch", "--quiet", "main"]);
	await run_git(root, ["clone", "--quiet", remote, workspace]);
	await run_git(workspace, ["config", "core.autocrlf", "false"]);
	await run_git(workspace, ["config", "user.email", "tests@example.com"]);
	await run_git(workspace, ["config", "user.name", "Artisan Tests"]);

	return { remote, source, workspace };
}

async function fetch_head_path(workspace: string) {
	const value = (
		await run_git(workspace, ["rev-parse", "--git-path", "FETCH_HEAD"])
	).stdout.trim();

	return resolve(workspace, value);
}

async function visible_state(workspace: string) {
	const fetch_head = await fetch_head_path(workspace);
	const [branch, head, index, status, worktrees, tracked, untracked, fetch_head_bytes] =
		await Promise.all([
			run_git(workspace, ["symbolic-ref", "--short", "HEAD"]),
			run_git(workspace, ["rev-parse", "HEAD"]),
			run_git(workspace, ["ls-files", "--stage", "-z"]),
			run_git(workspace, ["status", "--porcelain=v1", "-z", "-uall"]),
			run_git(workspace, ["worktree", "list", "--porcelain", "-z"]),
			fs.readFile(join(workspace, "tracked.txt")),
			fs.readFile(join(workspace, "untracked.txt")),
			fs.readFile(fetch_head),
		]);

	return {
		branch: branch.stdout,
		fetch_head_bytes,
		head: head.stdout,
		index: index.stdout,
		status: status.stdout,
		tracked,
		untracked,
		worktrees: worktrees.stdout,
	};
}

async function make_fetch(workspace: string) {
	const git_executable = await find_git_executable();
	const fetch = await Effect.runPromise(
		Effect.service(GitFetch).pipe(
			Effect.provide(
				make_node_git_fetch_layer({
					cwd: workspace,
					git_executable,
					fetch_timeout_ms: 30_000,
				}),
			),
		),
	);

	return { fetch, git_executable };
}

async function make_hooked_fetch(
	workspace: string,
	hooks: {
		readonly after?: (input: ProcessRunnerInput, result: ProcessRunnerResult) => Promise<void>;
		readonly before?: (input: ProcessRunnerInput) => Promise<void>;
	},
) {
	const git_executable = await find_git_executable();
	const base = await Effect.runPromise(
		Effect.service(ProcessRunner).pipe(Effect.provide(make_node_process_runner_layer())),
	);
	const runner = Layer.succeed(ProcessRunner, {
		Run: (input: ProcessRunnerInput) =>
			Effect.gen(function* () {
				if (hooks.before !== undefined) {
					yield* Effect.promise(() => hooks.before!(input));
				}

				const result = yield* base.Run(input);

				if (hooks.after !== undefined) {
					yield* Effect.promise(() => hooks.after!(input, result));
				}

				return result;
			}),
		RunProcessTree: (input) => base.Run(input),
	});
	const layer = make_git_fetch_layer({
		cwd: workspace,
		fetch_timeout_ms: 30_000,
		git_executable,
	}).pipe(
		Layer.provide(runner),
		Layer.provide(NodeCrypto.layer),
		Layer.provide(NodeFileSystem.layer),
		Layer.provide(NodePath.layer),
	);
	const fetch = await Effect.runPromise(Effect.service(GitFetch).pipe(Effect.provide(layer)));

	return { fetch, git_executable };
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitFetch", () => {
	it("atomically refreshes remote refs without changing visible checkout state", async () => {
		const { remote, source, workspace } = await make_remote_fixture();

		await fs.writeFile(join(workspace, "tracked.txt"), "staged local work\n");
		await run_git(workspace, ["add", "tracked.txt"]);
		await fs.writeFile(join(workspace, "untracked.txt"), "untracked local work\n");
		await fs.writeFile(await fetch_head_path(workspace), "artisan fetch-head sentinel\n");

		const before = await visible_state(workspace);

		await commit_file(source, "remote.txt", "remote update\n", "remote update");
		await run_git(source, ["push", "--quiet", "origin", "main"]);
		await run_git(source, ["push", "--quiet", "origin", "--delete", "feature"]);
		await run_git(source, ["switch", "--quiet", "--create", "release"]);
		await commit_file(source, "release.txt", "release\n", "release");
		await run_git(source, ["push", "--quiet", "--set-upstream", "origin", "release"]);

		const { fetch, git_executable } = await make_fetch(workspace);
		const authorization = {
			environment: {},
			git_executable_path: git_executable,
			remote_endpoint: remote,
			transport_protocol: "file" as const,
		};
		const result = await Effect.runPromise(
			fetch.Fetch({ remote: "origin", remote_endpoint: remote }, authorization),
		);
		const after = await visible_state(workspace);
		const refs = (
			await run_git(workspace, [
				"for-each-ref",
				"--format=%(refname):%(objectname)",
				"refs/remotes/origin/",
			])
		).stdout;

		expect(result).toEqual({
			created_refs: 1,
			deleted_refs: 1,
			remote: "origin",
			remote_refs: 2,
			updated_refs: 1,
		});
		expect(after).toEqual(before);
		expect(refs).toContain("refs/remotes/origin/main:");
		expect(refs).toContain("refs/remotes/origin/release:");
		expect(refs).not.toContain("refs/remotes/origin/feature:");

		await expect(
			Effect.runPromise(
				fetch.Fetch({ remote: "origin", remote_endpoint: remote }, authorization),
			),
		).resolves.toEqual({
			created_refs: 0,
			deleted_refs: 0,
			remote: "origin",
			remote_refs: 2,
			updated_refs: 0,
		});
	});

	it("rejects authorization minted for another endpoint before changing refs", async () => {
		const { remote, workspace } = await make_remote_fixture();
		const { fetch, git_executable } = await make_fetch(workspace);
		const before = (
			await run_git(workspace, ["for-each-ref", "--format=%(refname):%(objectname)"])
		).stdout;
		const failure = await Effect.runPromise(
			fetch
				.Fetch(
					{ remote: "origin", remote_endpoint: remote },
					{
						environment: {},
						git_executable_path: git_executable,
						remote_endpoint: join(dirname(remote), "other.git"),
						transport_protocol: "file",
					},
				)
				.pipe(Effect.flip),
		);
		const after = (
			await run_git(workspace, ["for-each-ref", "--format=%(refname):%(objectname)"])
		).stdout;

		expect(failure.operation).toBe("invalid_authorization");
		expect(after).toBe(before);
	});

	it("aborts every ref update when one remote-tracking ref races the transaction", async () => {
		const { remote, source, workspace } = await make_remote_fixture();

		await fs.writeFile(join(workspace, "tracked.txt"), "staged local work\n");
		await run_git(workspace, ["add", "tracked.txt"]);
		await fs.writeFile(join(workspace, "untracked.txt"), "untracked local work\n");
		await fs.writeFile(await fetch_head_path(workspace), "artisan fetch-head sentinel\n");

		const before = await visible_state(workspace);
		const feature_head = (
			await run_git(workspace, ["rev-parse", "refs/remotes/origin/feature"])
		).stdout.trim();

		await commit_file(source, "remote.txt", "remote update\n", "remote update");
		await run_git(source, ["push", "--quiet", "origin", "main"]);
		await run_git(source, ["switch", "--quiet", "--create", "release"]);
		await commit_file(source, "release.txt", "release\n", "release");
		await run_git(source, ["push", "--quiet", "--set-upstream", "origin", "release"]);

		let raced = false;
		const { fetch, git_executable } = await make_hooked_fetch(workspace, {
			before: async (input) => {
				if (!raced && input.args.includes("update-ref") && input.stdin !== undefined) {
					raced = true;
					await run_git(workspace, [
						"update-ref",
						"refs/remotes/origin/main",
						feature_head,
					]);
				}
			},
		});
		const failure = await Effect.runPromise(
			fetch
				.Fetch(
					{ remote: "origin", remote_endpoint: remote },
					{
						environment: {},
						git_executable_path: git_executable,
						remote_endpoint: remote,
						transport_protocol: "file",
					},
				)
				.pipe(Effect.flip),
		);
		const after = await visible_state(workspace);
		const raced_main = (
			await run_git(workspace, ["rev-parse", "refs/remotes/origin/main"])
		).stdout.trim();
		const release = await run_process(workspace, git_executable, [
			"rev-parse",
			"--verify",
			"--quiet",
			"refs/remotes/origin/release",
		]).then(
			() => true,
			() => false,
		);

		expect(raced).toBe(true);
		expect(failure.operation).toBe("settlement");
		expect(after).toEqual(before);
		expect(raced_main).toBe(feature_head);
		expect(release).toBe(false);
	});
});
