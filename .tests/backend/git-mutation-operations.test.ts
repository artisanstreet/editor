import { spawn } from "node:child_process";
import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GitMutation } from "../../modules/backend/src/git/git-mutation";
import { make_node_git_mutation_layer } from "../../modules/backend/src/git/node-git-mutation";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan-git-mutation-operations-"));

	roots.push(root);

	return root;
}

async function run_git(cwd: string, args: ReadonlyArray<string>) {
	const { stdout } = await run_process(cwd, ["git", ...args]);

	return stdout;
}

async function run_process(cwd: string, command: ReadonlyArray<string>) {
	const [file, ...args] = command;

	return new Promise<{ readonly stderr: string; readonly stdout: string }>((resolve, reject) => {
		const child = spawn(file!, args, {
			cwd,
			shell: false,
			stdio: ["ignore", "pipe", "pipe"],
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

			reject(new Error(`${command.join(" ")} exited ${code}: ${output.stderr}`));
		});
	});
}

async function initialize_repository(root: string, object_format: "sha1" | "sha256" = "sha1") {
	await run_git(root, ["init", "--quiet", `--object-format=${object_format}`]);
	await run_git(root, ["config", "core.autocrlf", "false"]);
	await run_git(root, ["config", "user.email", "tests@example.com"]);
	await run_git(root, ["config", "user.name", "Artisan Tests"]);
	await fs.writeFile(join(root, "tracked.txt"), "base\n");
	await run_git(root, ["add", "tracked.txt"]);
	await run_git(root, ["commit", "--quiet", "--message", "base"]);
}

async function commit_file(root: string, path: string, content: string, message: string) {
	await fs.writeFile(join(root, path), content);
	await run_git(root, ["add", "--", path]);
	await run_git(root, ["commit", "--quiet", "--message", message]);

	return read_head(root);
}

async function read_git(root: string, args: ReadonlyArray<string>) {
	return (await run_git(root, args)).trim();
}

async function read_head(root: string) {
	return read_git(root, ["rev-parse", "HEAD"]);
}

async function make_mutation(root: string) {
	return Effect.runPromise(
		Effect.service(GitMutation).pipe(
			Effect.provide(make_node_git_mutation_layer({ cwd: root })),
		),
	);
}

async function prepare_execute_reconcile(root: string, operation: unknown) {
	const mutation = await make_mutation(root);
	const plan = await Effect.runPromise(mutation.Prepare(operation));
	const attempt = await Effect.runPromise(mutation.Execute(plan));
	const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

	return { attempt, outcome, plan };
}

async function expect_visible_topology(root: string, branches: ReadonlyArray<string>) {
	const [worktrees, visible_branches] = await Promise.all([
		run_git(root, ["worktree", "list", "--porcelain"]),
		run_git(root, ["for-each-ref", "--format=%(refname:short)", "refs/heads"]),
	]);

	expect(worktrees.match(/^worktree /gmu)).toHaveLength(1);
	expect(visible_branches.trim().split("\n").sort()).toEqual([...branches].sort());
}

async function make_remote_repository() {
	const root = await make_root();
	const remote = join(root, "remote.git");
	const peer = join(root, "peer");
	const workspace = join(root, "workspace");

	await fs.mkdir(remote);
	await run_git(remote, ["init", "--bare", "--quiet"]);
	await fs.mkdir(workspace);
	await initialize_repository(workspace);
	await run_git(workspace, ["remote", "add", "origin", remote]);
	await run_git(workspace, ["push", "--quiet", "--set-upstream", "origin", "master"]);
	await run_git(root, ["clone", "--quiet", remote, peer]);
	await run_git(peer, ["config", "user.email", "tests@example.com"]);
	await run_git(peer, ["config", "user.name", "Artisan Tests"]);
	await run_git(peer, ["config", "core.autocrlf", "false"]);

	return { peer, remote, root, workspace };
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitMutation approved operations", () => {
	it.each(["soft", "mixed", "hard"] as const)(
		"applies a %s reset with Git's observable index and worktree semantics",
		async (mode) => {
			const root = await make_root();

			await initialize_repository(root);
			const base = await read_head(root);
			await commit_file(root, "tracked.txt", "second\n", "second");

			const { outcome, plan } = await prepare_execute_reconcile(root, {
				mode,
				target: base,
				type: "reset",
			});
			const [head, index, worktree] = await Promise.all([
				read_head(root),
				read_git(root, ["show", ":tracked.txt"]),
				fs.readFile(join(root, "tracked.txt"), "utf8"),
			]);

			expect(plan.type).toBe("reset");
			expect(outcome).toMatchObject({ head: base, type: "applied" });
			expect(head).toBe(base);
			expect(index).toBe(mode === "soft" ? "second" : "base");
			expect(worktree).toBe(mode === "hard" ? "base\n" : "second\n");
			await expect_visible_topology(root, ["master"]);
		},
	);

	it.each([
		["sha1", 40],
		["sha256", 64],
	] as const)(
		"checks out the exact approved %s branch object ID",
		async (object_format, length) => {
			const root = await make_root();

			await initialize_repository(root, object_format);
			await run_git(root, ["switch", "--quiet", "--create", "feature"]);
			const feature_head = await commit_file(root, "tracked.txt", "feature\n", "feature");
			await run_git(root, ["switch", "--quiet", "master"]);

			const { outcome, plan } = await prepare_execute_reconcile(root, {
				target_branch: "feature",
				type: "checkout",
			});

			expect(plan).toMatchObject({ target_head: feature_head, type: "checkout" });
			expect(feature_head).toHaveLength(length);
			expect(outcome).toMatchObject({
				branch: "feature",
				head: feature_head,
				type: "applied",
			});
			expect(await read_head(root)).toBe(feature_head);
			await expect_visible_topology(root, ["feature", "master"]);
		},
	);

	it("merges an approved branch and leaves only the visible branches and merge commit", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		await commit_file(root, "master.txt", "master\n", "master");
		await run_git(root, ["switch", "--quiet", "feature"]);
		await commit_file(root, "feature.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);

		const { outcome } = await prepare_execute_reconcile(root, {
			action: "start",
			target_branch: "feature",
			type: "merge",
		});

		expect(outcome.type).toBe("applied");
		expect(await read_git(root, ["log", "--format=%P", "-1"])).toMatch(
			/^[a-f0-9]+ [a-f0-9]+$/u,
		);
		expect(await read_git(root, ["rev-list", "--all", "--count"])).toBe("4");
		await expect_visible_topology(root, ["feature", "master"]);
	});

	it("reconciles a merge conflict and continues only with an anchored approval", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		await commit_file(root, "tracked.txt", "master\n", "master");
		const master_head = await read_head(root);
		await run_git(root, ["switch", "--quiet", "feature"]);
		await commit_file(root, "tracked.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);

		const mutation = await make_mutation(root);
		const start = await Effect.runPromise(
			mutation.Prepare({ action: "start", target_branch: "feature", type: "merge" }),
		);
		const attempt = await Effect.runPromise(mutation.Execute(start));
		const conflict = await Effect.runPromise(mutation.Reconcile(start, attempt));

		expect(conflict.type).toBe("action_required");
		expect(conflict).toMatchObject({ action: "merge_conflict" });
		if (conflict.type !== "action_required") throw new Error("expected merge conflict anchor");
		await fs.writeFile(join(root, "tracked.txt"), "resolved\n");
		await run_git(root, ["add", "tracked.txt"]);

		const continue_plan = await Effect.runPromise(
			mutation.Prepare({
				action_anchor: conflict.anchor,
				operation: { action: "continue", type: "merge" },
			}),
		);
		const continued = await Effect.runPromise(mutation.Execute(continue_plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(continue_plan, continued));

		expect(outcome.type).toBe("applied");
		expect(await fs.readFile(join(root, "tracked.txt"), "utf8")).toBe("resolved\n");
		expect(await read_git(root, ["rev-list", "--all", "--count"])).toBe("4");
		expect(await read_git(root, ["merge-base", "master", master_head])).toBe(master_head);
		await expect_visible_topology(root, ["feature", "master"]);
	});

	it("aborts an anchored merge conflict back to the approved source", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		const master_head = await commit_file(root, "tracked.txt", "master\n", "master");
		await run_git(root, ["switch", "--quiet", "feature"]);
		await commit_file(root, "tracked.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);

		const mutation = await make_mutation(root);
		const start = await Effect.runPromise(
			mutation.Prepare({ action: "start", target_branch: "feature", type: "merge" }),
		);
		const attempt = await Effect.runPromise(mutation.Execute(start));
		const conflict = await Effect.runPromise(mutation.Reconcile(start, attempt));

		if (conflict.type !== "action_required") throw new Error("expected merge conflict anchor");
		const abort_plan = await Effect.runPromise(
			mutation.Prepare({
				action_anchor: conflict.anchor,
				operation: { action: "abort", type: "merge" },
			}),
		);
		const aborted = await Effect.runPromise(mutation.Execute(abort_plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(abort_plan, aborted));

		expect(outcome).toMatchObject({ head: master_head, type: "applied" });
		expect(await read_head(root)).toBe(master_head);
		expect(await read_git(root, ["rev-list", "--all", "--count"])).toBe("3");
		await expect_visible_topology(root, ["feature", "master"]);
	});

	it("rebases an approved branch onto its target without creating a branch or worktree", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		await commit_file(root, "master.txt", "master\n", "master");
		await run_git(root, ["switch", "--quiet", "feature"]);
		await commit_file(root, "feature.txt", "feature\n", "feature");
		await run_git(root, ["switch", "--quiet", "master"]);

		const { outcome } = await prepare_execute_reconcile(root, {
			action: "start",
			target_branch: "feature",
			type: "rebase",
		});

		expect(outcome.type).toBe("applied");
		expect(await read_git(root, ["merge-base", "master", "feature"])).toBe(
			await read_git(root, ["rev-parse", "feature"]),
		);
		expect(await read_git(root, ["rev-list", "--all", "--count"])).toBe("3");
		await expect_visible_topology(root, ["feature", "master"]);
	});

	it.each(["continue", "abort", "skip"] as const)(
		"handles an anchored rebase conflict through %s",
		async (action) => {
			const root = await make_root();

			await initialize_repository(root);
			await run_git(root, ["branch", "feature"]);
			await commit_file(root, "tracked.txt", "master\n", "master conflict");
			if (action === "skip") {
				await commit_file(root, "after.txt", "after\n", "after conflict");
			}
			const original_head = await read_head(root);
			await run_git(root, ["switch", "--quiet", "feature"]);
			await commit_file(root, "tracked.txt", "feature\n", "feature conflict");
			const feature_head = await read_head(root);
			await run_git(root, ["switch", "--quiet", "master"]);

			const mutation = await make_mutation(root);
			const start = await Effect.runPromise(
				mutation.Prepare({ action: "start", target_branch: "feature", type: "rebase" }),
			);
			const attempt = await Effect.runPromise(mutation.Execute(start));
			const conflict = await Effect.runPromise(mutation.Reconcile(start, attempt));

			expect(conflict).toMatchObject({ action: "rebase_conflict", type: "action_required" });
			if (conflict.type !== "action_required")
				throw new Error("expected rebase conflict anchor");
			if (action === "continue") {
				await fs.writeFile(join(root, "tracked.txt"), "resolved\n");
				await run_git(root, ["add", "tracked.txt"]);
			}

			const action_plan = await Effect.runPromise(
				mutation.Prepare({
					action_anchor: conflict.anchor,
					operation: { action, type: "rebase" },
				}),
			);
			const action_attempt = await Effect.runPromise(mutation.Execute(action_plan));
			const outcome = await Effect.runPromise(
				mutation.Reconcile(action_plan, action_attempt),
			);

			expect(outcome.type).toBe("applied");
			if (action === "abort") {
				expect(await read_head(root)).toBe(original_head);
			} else {
				expect(await read_git(root, ["merge-base", "master", "feature"])).toBe(
					feature_head,
				);
			}
			if (action === "skip") {
				expect(await fs.readFile(join(root, "after.txt"), "utf8")).toBe("after\n");
			}
			await expect_visible_topology(root, ["feature", "master"]);
		},
	);

	it("fast-forwards only to the approved remote head", async () => {
		const { peer, workspace } = await make_remote_repository();

		const remote_head = await commit_file(peer, "remote.txt", "remote\n", "remote");
		await run_git(peer, ["push", "--quiet"]);

		const { outcome, plan } = await prepare_execute_reconcile(workspace, {
			type: "pull_ff_only",
		});

		expect(plan).toMatchObject({ type: "pull_ff_only", upstream_head: remote_head });
		expect(outcome).toMatchObject({ head: remote_head, type: "applied" });
		expect(await fs.readFile(join(workspace, "remote.txt"), "utf8")).toBe("remote\n");
		await expect_visible_topology(workspace, ["master"]);
	});

	it("rejects a pull whose approved remote head changed before execution", async () => {
		const { peer, workspace } = await make_remote_repository();

		await commit_file(peer, "remote.txt", "first\n", "first remote");
		await run_git(peer, ["push", "--quiet"]);
		const mutation = await make_mutation(workspace);
		const plan = await Effect.runPromise(mutation.Prepare({ type: "pull_ff_only" }));
		const changed_head = await commit_file(peer, "remote.txt", "second\n", "second remote");
		await run_git(peer, ["push", "--quiet"]);
		const failure = await Effect.runPromise(mutation.Execute(plan).pipe(Effect.flip));

		expect(failure.operation).toBe("precondition");
		expect(await read_head(workspace)).not.toBe(changed_head);
		expect(
			await fs.stat(join(workspace, "remote.txt")).then(
				() => true,
				() => false,
			),
		).toBe(false);
		await expect_visible_topology(workspace, ["master"]);
	});

	it("rejects a non-fast-forward pull without moving the local branch", async () => {
		const { peer, workspace } = await make_remote_repository();

		const local_head = await commit_file(workspace, "local.txt", "local\n", "local");
		await commit_file(peer, "remote.txt", "remote\n", "remote");
		await run_git(peer, ["push", "--quiet"]);

		const { attempt, outcome } = await prepare_execute_reconcile(workspace, {
			type: "pull_ff_only",
		});

		expect(attempt.rejection_reason).toBe("non_fast_forward");
		expect(outcome).toMatchObject({ reason: "non_fast_forward", type: "rejected" });
		expect(await read_head(workspace)).toBe(local_head);
		await expect_visible_topology(workspace, ["master"]);
	});

	it("pushes the exact approved object ID and can create a tracked remote branch", async () => {
		const { remote, workspace } = await make_remote_repository();

		await run_git(workspace, ["switch", "--quiet", "--create", "feature"]);
		const source_head = await commit_file(workspace, "feature.txt", "feature\n", "feature");
		const { outcome, plan } = await prepare_execute_reconcile(workspace, {
			remote: "origin",
			set_upstream: true,
			target_branch: "published",
			type: "push",
		});

		expect(plan).toMatchObject({ source_head, type: "push" });
		expect("expected_remote_head" in plan).toBe(false);
		expect(outcome).toMatchObject({
			head: source_head,
			remote: "origin",
			remote_endpoint: remote,
			remote_head: source_head,
			target_branch: "published",
			type: "applied",
		});
		expect(await read_git(remote, ["rev-parse", "refs/heads/published"])).toBe(source_head);
		expect(
			await read_git(workspace, [
				"rev-parse",
				"--abbrev-ref",
				"--symbolic-full-name",
				"@{upstream}",
			]),
		).toBe("origin/published");
		await expect_visible_topology(workspace, ["feature", "master"]);
	});

	it("rejects a stale push lease without changing the remote branch", async () => {
		const { peer, remote, workspace } = await make_remote_repository();

		const local_head = await commit_file(workspace, "local.txt", "local\n", "local");
		const mutation = await make_mutation(workspace);
		const plan = await Effect.runPromise(
			mutation.Prepare({
				remote: "origin",
				set_upstream: false,
				target_branch: "master",
				type: "push",
			}),
		);
		const peer_head = await commit_file(peer, "peer.txt", "peer\n", "peer");
		await run_git(peer, ["push", "--quiet"]);
		const failure = await Effect.runPromise(mutation.Execute(plan).pipe(Effect.flip));

		expect(failure.operation).toBe("precondition");
		expect(await read_git(remote, ["rev-parse", "refs/heads/master"])).toBe(peer_head);
		expect(await read_head(workspace)).toBe(local_head);
		await expect_visible_topology(workspace, ["master"]);
	});
});
