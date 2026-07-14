import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	Git,
	type GitFileSummary,
	type GitRepository,
	type GitWorktree,
} from "../../modules/backend/src/git/git";
import {
	WorkspaceGitRegistry,
	make_node_workspace_git_registry_layer,
} from "../../modules/backend/src/git/workspace-git-registry";
import {
	WorkspaceGitObservationError,
	WorkspaceGitObserver,
	WorkspaceGitObserverLive,
} from "../../modules/backend/src/git/workspace-git-observer";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const roots: Array<string> = [];
const observed_at = "2026-07-13T00:00:00.000Z";

async function make_root(prefix = "artisan workspace git observer ") {
	const root = await fs.mkdtemp(join(tmpdir(), prefix));

	roots.push(root);

	return root;
}

async function run_git(cwd: string, args: ReadonlyArray<string>) {
	const { spawn } = await import("node:child_process");

	await new Promise<void>((resolve, reject) => {
		const child = spawn("git", [...args], { cwd, shell: false, stdio: "ignore" });

		child.on("error", reject);
		child.on("close", (code) =>
			code === 0 ? resolve() : reject(new Error(`git exited ${code}`)),
		);
	});
}

async function initialize_repository(root: string, commit = true) {
	await run_git(root, ["init", "-q"]);
	await run_git(root, ["config", "user.email", "test@example.com"]);
	await run_git(root, ["config", "user.name", "Test User"]);

	if (commit) {
		await fs.writeFile(join(root, "tracked.txt"), "initial\n");
		await run_git(root, ["add", "."]);
		await run_git(root, ["commit", "-qm", "initial"]);
	}
}

function observer_for(registrations: ReadonlyArray<unknown>) {
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_test",
		MakeId: (prefix) => Effect.succeed(`${prefix}_test`),
		Now: Effect.succeed(observed_at),
	});

	return Effect.service(WorkspaceGitObserver).pipe(
		Effect.provide(
			WorkspaceGitObserverLive.pipe(
				Layer.provide(make_node_workspace_git_registry_layer(registrations)),
				Layer.provide(metadata),
				Layer.provide(NodeFileSystem.layer),
			),
		),
	);
}

async function observe(root: string, workspace_id = "workspace-a") {
	const observer = await Effect.runPromise(observer_for([{ root, workspace_id }]));

	return Effect.runPromise(observer.Observe(workspace_id));
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceGitObserver", () => {
	it("projects a non-Git root as unavailable without exposing path details", async () => {
		const root = await make_root();
		const observation = await observe(root);

		expect(observation).toMatchObject({
			blockers: ["not_repository"],
			diff_stats: { additions: 0, deletions: 0, files: 0 },
			has_diff: false,
			observed_at,
			state: "unavailable",
			workspace_id: "workspace-a",
			worktrees: [],
		});
		expect(observation.repository_root).toBeUndefined();
		expect(JSON.stringify(observation)).not.toContain(root);
	});

	it("projects a clean single worktree as ready with branch and head", async () => {
		const root = await make_root();
		await initialize_repository(root);

		const observation = await observe(root);

		expect(observation.state).toBe("ready");
		expect(observation.blockers).toEqual([]);
		expect(observation.branch).toBe("master");
		expect(observation.head).toMatch(/^[0-9a-f]{40}$/);
		expect(observation.repository_root).toBe(await fs.realpath(root));
		expect(observation.selected_worktree_path).toBe(await fs.realpath(root));
		expect(observation.worktrees).toHaveLength(1);
		expect(observation.worktrees[0]).toMatchObject({
			branch: "master",
			detached: false,
			location: "selected",
		});
	});

	it("maps staged, unstaged, untracked, and conflicted files", async () => {
		const root = await make_root();
		await initialize_repository(root);
		await fs.writeFile(join(root, "staged.txt"), "base\n");
		await fs.writeFile(join(root, "unstaged.txt"), "base\n");
		await run_git(root, ["add", "staged.txt", "unstaged.txt"]);
		await run_git(root, ["commit", "-qm", "status base"]);

		await fs.writeFile(join(root, "conflict.txt"), "base\n");
		await run_git(root, ["add", "conflict.txt"]);
		await run_git(root, ["commit", "-qm", "conflict base"]);
		await run_git(root, ["checkout", "-qb", "feature"]);
		await fs.writeFile(join(root, "conflict.txt"), "feature\n");
		await run_git(root, ["commit", "-qam", "feature conflict"]);
		await run_git(root, ["checkout", "-q", "master"]);
		await fs.writeFile(join(root, "conflict.txt"), "master\n");
		await run_git(root, ["commit", "-qam", "master conflict"]);
		await run_git(root, ["merge", "feature"]).catch(() => undefined);
		await fs.writeFile(join(root, "staged.txt"), "staged\n");
		await run_git(root, ["add", "staged.txt"]);
		await fs.writeFile(join(root, "unstaged.txt"), "unstaged\n");
		await fs.writeFile(join(root, "untracked.txt"), "untracked\n");

		const observation = await observe(root);
		const by_path = new Map(observation.changed_files.map((file) => [file.path, file]));

		expect(observation.state).toBe("ready");
		expect(by_path.get("staged.txt")).toMatchObject({ staged: true, unstaged: false });
		expect(by_path.get("unstaged.txt")).toMatchObject({ unstaged: true, staged: false });
		expect(by_path.get("untracked.txt")).toMatchObject({ untracked: true });
		expect(by_path.get("conflict.txt")).toMatchObject({ conflicted: true });
		expect(observation.has_diff).toBe(true);
		expect(observation.diff_stats.files).toBeGreaterThan(0);
	});

	it("blocks multiple, detached, locked, and prunable worktrees without public paths", async () => {
		const root = await make_root();
		const locked_root = await make_root();
		const detached_root = await make_root();
		const prunable_root = await make_root();
		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		await run_git(root, ["worktree", "add", "-q", locked_root, "feature"]);
		await run_git(root, ["worktree", "add", "-q", "--detach", detached_root, "HEAD"]);
		await run_git(root, ["worktree", "add", "-q", "-b", "prunable", prunable_root]);
		await run_git(root, ["worktree", "lock", "--reason", "test lock", locked_root]);
		await fs.rm(prunable_root, { force: true, recursive: true });

		const observation = await observe(root);

		expect(observation.state).toBe("blocked");
		expect(observation.blockers).toEqual(
			expect.arrayContaining([
				"multiple_worktrees",
				"detached_head",
				"locked_worktree",
				"prunable_worktree",
			]),
		);
		expect(observation.worktrees.every((worktree) => !("adapter_path" in worktree))).toBe(true);
		expect(JSON.stringify(observation)).not.toContain(locked_root);
		expect(JSON.stringify(observation)).not.toContain(detached_root);
	});

	it("blocks an unborn head", async () => {
		const root = await make_root();
		await initialize_repository(root, false);

		const observation = await observe(root);

		expect(observation.state).toBe("blocked");
		expect(observation.blockers).toContain("unborn_head");
		expect(observation.head).toBeUndefined();
	});

	it("reports selected-root and repository mismatches", async () => {
		const registered_root = await make_root();
		const repository_root = await make_root();
		await initialize_repository(repository_root);

		const read: typeof Git.Service = {
			DiffPatch: () => Effect.die("unexpected mutation-like read"),
			DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
			Discover: Effect.succeed({
				branch: "master",
				head: Option.some("a".repeat(40)),
				root: repository_root,
			}),
			ProbeRepository: Effect.succeed(
				Option.some({
					branch: "master",
					head: Option.some("a".repeat(40)),
					root: repository_root,
				}),
			),
			ResolveLocalBranch: () => Effect.succeed(Option.none()),
			Status: Effect.succeed([]),
			Worktrees: Effect.succeed([
				{
					adapter_path: repository_root,
					bare: false,
					branch: Option.some("refs/heads/master"),
					detached: false,
					head: Option.some("a".repeat(40)),
					locked: false,
					prunable: false,
				} satisfies GitWorktree,
			]),
		};
		const mutation = {
			Execute: () => Effect.die("observer invoked GitMutation"),
			Prepare: () => Effect.die("observer invoked GitMutation"),
			Reconcile: () => Effect.die("observer invoked GitMutation"),
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Get: () =>
				Effect.succeed({
					canonical_root: registered_root,
					mutation,
					read,
					workspace_id: "workspace-a",
				}),
			ListWorkspaceIds: Effect.succeed(["workspace-a"]),
		});
		const metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "backend_test",
			MakeId: (prefix) => Effect.succeed(`${prefix}_test`),
			Now: Effect.succeed(observed_at),
		});
		const observer = await Effect.runPromise(
			Effect.service(WorkspaceGitObserver).pipe(
				Effect.provide(
					WorkspaceGitObserverLive.pipe(
						Layer.provide(registry),
						Layer.provide(metadata),
						Layer.provide(NodeFileSystem.layer),
					),
				),
			),
		);

		const observation = await Effect.runPromise(observer.Observe("workspace-a"));

		expect(observation.state).toBe("blocked");
		expect(observation.blockers).toEqual(
			expect.arrayContaining(["selected_worktree_missing", "selected_worktree_mismatch"]),
		);
	});

	it("fails closed when the repository changes between probe and discover", async () => {
		const root = await make_root();
		const first: GitRepository = {
			branch: "master",
			head: Option.some("a".repeat(40)),
			root,
		};
		const second: GitRepository = { ...first, head: Option.some("b".repeat(40)) };
		const read: typeof Git.Service = {
			DiffPatch: () => Effect.die("unexpected diff patch"),
			DiffStats: Effect.succeed({ additions: 0, deletions: 0, files: 0 }),
			Discover: Effect.succeed(second),
			ProbeRepository: Effect.succeed(Option.some(first)),
			ResolveLocalBranch: () => Effect.succeed(Option.none()),
			Status: Effect.succeed([] as ReadonlyArray<GitFileSummary>),
			Worktrees: Effect.succeed([]),
		};
		const registry = Layer.succeed(WorkspaceGitRegistry, {
			Get: () =>
				Effect.succeed({
					canonical_root: root,
					mutation: {
						Execute: () => Effect.die("unexpected mutation"),
						Prepare: () => Effect.die("unexpected mutation"),
						Reconcile: () => Effect.die("unexpected mutation"),
					},
					read,
					workspace_id: "workspace-a",
				}),
			ListWorkspaceIds: Effect.succeed(["workspace-a"]),
		});
		const metadata = Layer.succeed(RuntimeMetadata, {
			instance_id: "backend_test",
			MakeId: (prefix) => Effect.succeed(`${prefix}_test`),
			Now: Effect.succeed(observed_at),
		});
		const observer = await Effect.runPromise(
			Effect.service(WorkspaceGitObserver).pipe(
				Effect.provide(
					WorkspaceGitObserverLive.pipe(
						Layer.provide(registry),
						Layer.provide(metadata),
						Layer.provide(NodeFileSystem.layer),
					),
				),
			),
		);
		const failure = await Effect.runPromise(observer.Observe("workspace-a").pipe(Effect.flip));

		expect(failure).toBeInstanceOf(WorkspaceGitObservationError);
		expect(failure.reason).toBe("invalid_state");
	});
});
