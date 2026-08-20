import { promises as fs } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { NodeCrypto } from "@effect/platform-node-shared";
import { Effect, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitReadService,
	make_git_read_service_layer,
	type GitReadServiceOptions,
} from "../../modules/backend/src/git/read-service";
import { type GitCommandResult } from "../../modules/backend/src/git/executor";
import {
	make_node_workspace_git_registry_layer,
	type WorkspaceGitCommandInput,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";

const roots: Array<string> = [];

async function make_container() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-read-service-"));

	roots.push(root);

	return root;
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

async function make_read_service(root: string, options: GitReadServiceOptions = {}) {
	const registry = make_node_workspace_git_registry_layer([
		{ root, workspace_id: "workspace_one" },
	]);
	const read = make_git_read_service_layer(options).pipe(
		Layer.provideMerge(registry),
		Layer.provideMerge(NodeCrypto.layer),
	);

	return Effect.runPromise(Effect.service(GitReadService).pipe(Effect.provide(read)));
}

describe("GitReadService", () => {
	it("refreshes one coherent attached snapshot with three diff scopes and worktree identity", async () => {
		const container = await make_container();
		const root = join(container, "repository");
		const linked = join(container, "linked checkout");

		await fs.mkdir(root);
		await initialize_repository(root);
		await fs.writeFile(join(root, "staged.txt"), "base\n");
		await fs.writeFile(join(root, "unstaged.txt"), "base\n");
		await run_git(root, ["add", "."]);
		await run_git(root, ["commit", "-qm", "initial"]);
		await fs.writeFile(join(root, "staged.txt"), "staged\n");
		await run_git(root, ["add", "staged.txt"]);
		await fs.writeFile(join(root, "unstaged.txt"), "unstaged\n");
		await fs.writeFile(join(root, "--odd name.txt"), "untracked\n");
		await run_git(root, ["worktree", "add", "--detach", linked, "HEAD"]);

		const read = await make_read_service(root);
		const first = await Effect.runPromise(read.Refresh("workspace_one"));
		const second = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(first.head._tag).toBe("attached");
		expect(first.files).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ path: "staged.txt", staged: true }),
				expect.objectContaining({ path: "unstaged.txt", unstaged: true }),
				expect.objectContaining({ path: "--odd name.txt", untracked: true }),
			]),
		);
		expect(first.aggregate).toEqual({
			additions: 2,
			binary_files: 0,
			deletions: 2,
			files: 2,
		});
		expect(first.staged).toEqual({ additions: 1, binary_files: 0, deletions: 1, files: 1 });
		expect(first.unstaged).toEqual({
			additions: 1,
			binary_files: 0,
			deletions: 1,
			files: 1,
		});
		expect(first.worktrees).toHaveLength(2);
		expect(first.worktrees.filter((worktree) => worktree.current)).toHaveLength(1);
		const current_root = first.worktrees.find((worktree) => worktree.current)?.path;
		expect(current_root).toBeDefined();
		const [current_root_stat, registered_root_stat] = await Promise.all([
			fs.stat(current_root!),
			fs.stat(root),
		]);
		expect({ dev: current_root_stat.dev, ino: current_root_stat.ino }).toEqual({
			dev: registered_root_stat.dev,
			ino: registered_root_stat.ino,
		});
		expect(first.snapshot_id).toBe(second.snapshot_id);

		await fs.writeFile(join(root, "--odd name.txt"), "changed untracked content\n");

		const untracked_changed = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(untracked_changed.snapshot_id).not.toBe(first.snapshot_id);

		await fs.writeFile(join(root, "unstaged.txt"), "changed again\n");

		const changed = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(changed.snapshot_id).not.toBe(untracked_changed.snapshot_id);
	});

	it("reads aggregate, staged, and unstaged patches with an explicit truncation result", async () => {
		const container = await make_container();
		const root = join(container, "repository");

		await fs.mkdir(root);
		await initialize_repository(root);
		await fs.writeFile(join(root, "staged.txt"), "base\n");
		await fs.writeFile(join(root, "unstaged.txt"), "base\n");
		await run_git(root, ["add", "."]);
		await run_git(root, ["commit", "-qm", "initial"]);
		await fs.writeFile(join(root, "staged.txt"), "staged change\n");
		await run_git(root, ["add", "staged.txt"]);
		await fs.writeFile(join(root, "unstaged.txt"), "unstaged change\n");

		const read = await make_read_service(root);
		const staged = await Effect.runPromise(
			read.ReadPatch({ scope: "staged", workspace_id: "workspace_one" }),
		);
		const unstaged = await Effect.runPromise(
			read.ReadPatch({ scope: "unstaged", workspace_id: "workspace_one" }),
		);
		const aggregate = await Effect.runPromise(
			read.ReadPatch({ scope: "all", workspace_id: "workspace_one" }),
		);
		const bounded = await Effect.runPromise(
			read.ReadPatch({ max_bytes: 17, scope: "all", workspace_id: "workspace_one" }),
		);

		expect(staged.patch).toContain("staged.txt");
		expect(staged.patch).not.toContain("diff --git a/unstaged.txt");
		expect(unstaged.patch).toContain("unstaged.txt");
		expect(unstaged.patch).not.toContain("diff --git a/staged.txt");
		expect(aggregate.patch).toContain("staged.txt");
		expect(aggregate.patch).toContain("unstaged.txt");
		expect(bounded.bytes).toBeLessThanOrEqual(17);
		expect(bounded.truncated).toBe(true);
	});

	it("represents unborn and detached HEAD states in live snapshots", async () => {
		const container = await make_container();
		const root = join(container, "repository");

		await fs.mkdir(root);
		await initialize_repository(root);

		const read = await make_read_service(root);
		const unborn = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(unborn.head._tag).toBe("unborn");

		await fs.writeFile(join(root, "file.txt"), "content\n");
		await run_git(root, ["add", "file.txt"]);
		await run_git(root, ["commit", "-qm", "initial"]);
		await run_git(root, ["checkout", "--detach", "-q"]);

		const detached = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(detached.head._tag).toBe("detached");
	});

	it("preserves a literal backslash in a POSIX-style worktree identity", async () => {
		const oid = "1".repeat(40);
		const root = "/tmp/literal\\worktree";
		const status = `# branch.oid ${oid}\0# branch.head main\0`;
		const worktrees = `worktree ${root}\0HEAD ${oid}\0branch refs/heads/main\0\0`;
		const Run = (input: WorkspaceGitCommandInput) => {
			const stdout = input.args.includes("--is-inside-work-tree")
				? "true\n"
				: input.args.includes("status")
					? status
					: input.args.includes("worktree")
						? worktrees
						: "";

			return Effect.succeed(command_result(stdout));
		};
		const capability = {
			git: { IsCurrentRoot: (path: string) => Effect.succeed(path === root), root, Run },
			workspace_id: "workspace_one",
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Authorize: () => Effect.succeed(capability),
			Get: () => Effect.succeed(capability),
			ListWorkspaceIds: Effect.succeed(["workspace_one"]),
			Reconcile: () => Effect.succeed([]),
			Register: () => Effect.succeed({ workspace_id: "workspace_one" }),
		});
		const read = await Effect.runPromise(
			Effect.service(GitReadService).pipe(
				Effect.provide(
					make_git_read_service_layer().pipe(
						Layer.provideMerge(registry),
						Layer.provideMerge(NodeCrypto.layer),
					),
				),
			),
		);
		const projection = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(projection.root).toBe(root);
		expect(projection.worktrees).toEqual([
			expect.objectContaining({ current: true, path: root }),
		]);
	});

	it("identifies the canonical root when the Git inventory reports an alias", async () => {
		const container = await make_container();
		const root = join(container, "repository");
		const alias = join(container, "repository-alias");
		const oid = "1".repeat(40);
		const status = `# branch.oid ${oid}\0# branch.head main\0`;
		const worktrees = `worktree ${alias}\0HEAD ${oid}\0branch refs/heads/main\0\0`;

		await fs.mkdir(root);
		await fs.symlink(root, alias, process.platform === "win32" ? "junction" : "dir");

		const Run = (input: WorkspaceGitCommandInput) =>
			Effect.succeed(
				command_result(
					input.args.includes("--is-inside-work-tree")
						? "true\n"
						: input.args.includes("status")
							? status
							: input.args.includes("worktree")
								? worktrees
								: "",
				),
			);
		const capability = {
			git: {
				IsCurrentRoot: (path: string) =>
					Effect.promise(() =>
						Promise.all([fs.realpath(path), fs.realpath(root)]).then(
							([candidate, expected]) => candidate === expected,
						),
					),
				root,
				Run,
			},
			workspace_id: "workspace_one",
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Authorize: () => Effect.succeed(capability),
			Get: () => Effect.succeed(capability),
			ListWorkspaceIds: Effect.succeed(["workspace_one"]),
			Reconcile: () => Effect.succeed([]),
			Register: () => Effect.succeed({ workspace_id: "workspace_one" }),
		});
		const read = await Effect.runPromise(
			Effect.service(GitReadService).pipe(
				Effect.provide(
					make_git_read_service_layer().pipe(
						Layer.provideMerge(registry),
						Layer.provideMerge(NodeCrypto.layer),
					),
				),
			),
		);

		const projection = await Effect.runPromise(read.Refresh("workspace_one"));

		expect(projection.worktrees).toEqual([
			expect.objectContaining({ current: true, path: alias }),
		]);
	});

	it("distinguishes a non-repository and fails closed when status output is truncated", async () => {
		const container = await make_container();
		const plain = join(container, "plain");
		const repository = join(container, "repository");

		await fs.mkdir(plain);
		await fs.mkdir(repository);
		await initialize_repository(repository);

		const plain_read = await make_read_service(plain);
		const not_repository = await Effect.runPromise(
			plain_read.Refresh("workspace_one").pipe(Effect.flip),
		);

		expect(not_repository.reason).toBe("not_repository");

		const bounded_read = await make_read_service(repository, { max_status_bytes: 16 });
		const truncated = await Effect.runPromise(
			bounded_read.Refresh("workspace_one").pipe(Effect.flip),
		);

		expect(truncated.reason).toBe("output_limit");

		const bounded_worktrees = await make_read_service(repository, { max_worktree_bytes: 8 });
		const worktree_truncated = await Effect.runPromise(
			bounded_worktrees.Refresh("workspace_one").pipe(Effect.flip),
		);

		expect(worktree_truncated.reason).toBe("output_limit");
	});

	it("rejects a snapshot when status changes during its bounded sampling window", async () => {
		const oid = "1".repeat(40);
		const initial_status = `# branch.oid ${oid}\0# branch.head main\0`;
		const changed_status = `${initial_status}? external.txt\0`;
		const worktrees = `worktree C:/repository\0HEAD ${oid}\0branch refs/heads/main\0\0`;
		let status_reads = 0;
		const Run = (input: WorkspaceGitCommandInput) => {
			let stdout = "";

			if (input.args.includes("--is-inside-work-tree")) {
				stdout = "true\n";
			} else if (input.args.includes("status")) {
				status_reads += 1;
				stdout = status_reads === 1 ? initial_status : changed_status;
			} else if (input.args.includes("worktree")) {
				stdout = worktrees;
			}

			return Effect.succeed(command_result(stdout));
		};
		const capability = {
			git: {
				IsCurrentRoot: (path: string) => Effect.succeed(path === "C:/repository"),
				root: "C:/repository",
				Run,
			},
			workspace_id: "workspace_one",
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Authorize: () => Effect.succeed(capability),
			Get: () => Effect.succeed(capability),
			ListWorkspaceIds: Effect.succeed(["workspace_one"]),
			Reconcile: () => Effect.succeed([]),
			Register: () => Effect.succeed({ workspace_id: "workspace_one" }),
		});
		const read_layer = make_git_read_service_layer().pipe(
			Layer.provideMerge(registry),
			Layer.provideMerge(NodeCrypto.layer),
		);
		const read = await Effect.runPromise(
			Effect.service(GitReadService).pipe(Effect.provide(read_layer)),
		);
		const error = await Effect.runPromise(read.Refresh("workspace_one").pipe(Effect.flip));
		expect(error.reason).toBe("snapshot_changed");
	});

	it("rejects a snapshot when tracked content changes without changing porcelain status", async () => {
		const oid = "1".repeat(40);
		const index_oid = "2".repeat(40);
		const status = [
			`# branch.oid ${oid}`,
			"# branch.head main",
			`1 M. N... 100644 100644 100644 ${oid} ${index_oid} tracked.txt`,
			"",
		].join("\0");
		const worktrees = `worktree C:/repository\0HEAD ${oid}\0branch refs/heads/main\0\0`;
		let content_reads = 0;
		const Run = (input: WorkspaceGitCommandInput) => {
			let stdout = "";

			if (input.args.includes("--is-inside-work-tree")) {
				stdout = "true\n";
			} else if (input.args.includes("status")) {
				stdout = status;
			} else if (input.args.includes("worktree")) {
				stdout = worktrees;
			} else if (input.args.includes("--binary")) {
				content_reads += 1;
				stdout = content_reads <= 3 ? "initial diff" : "changed diff";
			}

			return Effect.succeed(command_result(stdout));
		};
		const capability = {
			git: {
				IsCurrentRoot: (path: string) => Effect.succeed(path === "C:/repository"),
				root: "C:/repository",
				Run,
			},
			workspace_id: "workspace_one",
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Authorize: () => Effect.succeed(capability),
			Get: () => Effect.succeed(capability),
			ListWorkspaceIds: Effect.succeed(["workspace_one"]),
			Reconcile: () => Effect.succeed([]),
			Register: () => Effect.succeed({ workspace_id: "workspace_one" }),
		});
		const read_layer = make_git_read_service_layer().pipe(
			Layer.provideMerge(registry),
			Layer.provideMerge(NodeCrypto.layer),
		);
		const read = await Effect.runPromise(
			Effect.service(GitReadService).pipe(Effect.provide(read_layer)),
		);
		const error = await Effect.runPromise(read.Refresh("workspace_one").pipe(Effect.flip));

		expect(error.reason).toBe("snapshot_changed");
	});
});

function command_result(stdout: string): GitCommandResult {
	const bytes = new TextEncoder().encode(stdout);

	return {
		exit_code: 0,
		stderr: { bytes: new Uint8Array(), total_bytes: 0, truncated: false },
		stdout: { bytes, total_bytes: bytes.byteLength, truncated: false },
	};
}

async function initialize_repository(root: string) {
	await run_git(root, ["init", "-q"]);
	await run_git(root, ["config", "user.email", "test@example.com"]);
	await run_git(root, ["config", "user.name", "Test User"]);
}

async function run_git(cwd: string, args: ReadonlyArray<string>) {
	const { spawn } = await import("node:child_process");

	await new Promise<void>((resolve, reject) => {
		const child = spawn("git", [...args], {
			cwd,
			env: { ...process.env, GIT_TERMINAL_PROMPT: "0", LC_ALL: "C" },
			shell: false,
			stdio: "ignore",
		});

		child.on("error", reject);
		child.on("close", (code) =>
			code === 0 ? resolve() : reject(new Error(`git exited ${code}`)),
		);
	});
}
