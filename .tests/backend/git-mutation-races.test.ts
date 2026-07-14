import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Exit, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { GitMutation } from "../../modules/backend/src/git/git-mutation";
import { make_git_mutation_layer } from "../../modules/backend/src/git/node-git-mutation";
import { make_node_process_runner_layer } from "../../modules/backend/src/git/node-process-runner";
import {
	ProcessRunner,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan git mutation race "));

	roots.push(root);

	return root;
}

async function run_git(
	cwd: string,
	args: ReadonlyArray<string>,
	allowed_exit_codes: ReadonlyArray<number> = [0],
) {
	const { spawn } = await import("node:child_process");

	return new Promise<{ readonly exit_code: number; readonly stdout: string }>(
		(resolve, reject) => {
			const child = spawn("git", [...args], {
				cwd,
				shell: false,
				stdio: ["ignore", "pipe", "ignore"],
			});
			const chunks: Array<Buffer> = [];

			child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
			child.on("error", reject);
			child.on("close", (code) => {
				const exit_code = code ?? -1;

				if (!allowed_exit_codes.includes(exit_code)) {
					reject(new Error(`git exited ${exit_code}: ${args.join(" ")}`));
					return;
				}

				resolve({ exit_code, stdout: Buffer.concat(chunks).toString("utf8") });
			});
		},
	);
}

async function initialize_repository(root: string) {
	await run_git(root, ["init", "-q"]);
	await run_git(root, ["config", "core.autocrlf", "false"]);
	await run_git(root, ["config", "user.email", "test@example.com"]);
	await run_git(root, ["config", "user.name", "Test User"]);
	await fs.writeFile(join(root, "tracked.txt"), "initial\n");
	await run_git(root, ["add", "."]);
	await run_git(root, ["commit", "-qm", "initial"]);
}

async function read_git(cwd: string, args: ReadonlyArray<string>) {
	return (await run_git(cwd, args)).stdout.trim();
}

async function make_hooked_mutation(
	root: string,
	hooks: {
		readonly after?: (input: ProcessRunnerInput, result: ProcessRunnerResult) => Promise<void>;
		readonly before?: (input: ProcessRunnerInput) => Promise<void>;
		readonly transform?: (
			input: ProcessRunnerInput,
			result: ProcessRunnerResult,
		) => ProcessRunnerResult;
	},
	options: { readonly max_stdout_bytes?: number } = {},
) {
	const base_runner = await Effect.runPromise(
		Effect.service(ProcessRunner).pipe(Effect.provide(make_node_process_runner_layer())),
	);
	const runner = Layer.succeed(ProcessRunner, {
		Run: (input: ProcessRunnerInput) =>
			Effect.gen(function* () {
				if (hooks.before !== undefined) {
					yield* Effect.promise(() => hooks.before!(input));
				}

				const result = yield* base_runner.Run(input);

				if (hooks.after !== undefined) {
					yield* Effect.promise(() => hooks.after!(input, result));
				}

				return hooks.transform?.(input, result) ?? result;
			}),
	});
	const layer = make_git_mutation_layer({ cwd: root, ...options }).pipe(
		Layer.provide(runner),
		Layer.provide(NodeCrypto.layer),
		Layer.provide(NodeFileSystem.layer),
		Layer.provide(NodePath.layer),
	);

	return Effect.runPromise(Effect.service(GitMutation).pipe(Effect.provide(layer)));
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitMutation race reconciliation", () => {
	it("bounds accumulated output across a multi-process mutation", async () => {
		const root = await make_root();
		const filler = new Uint8Array(300).fill(65);

		await initialize_repository(root);

		const mutation = await make_hooked_mutation(
			root,
			{
				transform: (input, result) =>
					input.args.includes("switch") || input.args.includes("update-ref")
						? {
								...result,
								stdout: filler,
								stdout_bytes: filler.byteLength,
								stdout_truncated: false,
							}
						: result,
			},
			{ max_stdout_bytes: 512 },
		);
		const plan = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }),
		);
		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(attempt.output_complete).toBe(false);
		expect(outcome.type).toBe("outcome_unknown");
	});

	it("does not report a push as applied when upstream settlement fails", async () => {
		const parent = await make_root();
		const remote = join(parent, "remote.git");
		const workspace = join(parent, "workspace");

		await run_git(parent, ["init", "--bare", "-q", remote]);
		await fs.mkdir(workspace);
		await initialize_repository(workspace);
		await run_git(workspace, ["remote", "add", "origin", remote]);

		const mutation = await make_hooked_mutation(workspace, {
			transform: (input, result) =>
				input.args.includes("config") &&
				input.args.includes("--replace-all") &&
				input.args.some((argument) => argument.endsWith(".merge"))
					? { ...result, exit_code: 1 }
					: result,
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({
				remote: "origin",
				set_upstream: true,
				target_branch: "published",
				type: "push",
			}),
		);

		if (plan.type !== "push") {
			throw new Error("Expected a push plan");
		}

		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(attempt.operation_head).toBeUndefined();
		expect(outcome.type).toBe("outcome_unknown");
		expect(await read_git(remote, ["rev-parse", "refs/heads/published"])).toBe(
			plan.source_head,
		);
	});

	it("does not follow a symbolic remote-tracking ref during push settlement", async () => {
		const parent = await make_root();
		const remote = join(parent, "remote.git");
		const workspace = join(parent, "workspace");
		let raced = false;

		await run_git(parent, ["init", "--bare", "-q", remote]);
		await fs.mkdir(workspace);
		await initialize_repository(workspace);
		await run_git(workspace, ["remote", "add", "origin", remote]);

		const source_head = await read_git(workspace, ["rev-parse", "HEAD"]);
		const mutation = await make_hooked_mutation(workspace, {
			before: async (input) => {
				if (!raced && input.args.includes("update-ref") && input.stdin !== undefined) {
					raced = true;
					await run_git(workspace, [
						"symbolic-ref",
						"refs/remotes/origin/published",
						"refs/heads/master",
					]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({
				remote: "origin",
				set_upstream: true,
				target_branch: "published",
				type: "push",
			}),
		);
		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(raced).toBe(true);
		expect(attempt.operation_head).toBeUndefined();
		expect(outcome.type).toBe("outcome_unknown");
		expect(await read_git(workspace, ["rev-parse", "refs/heads/master"])).toBe(source_head);
		expect(await read_git(workspace, ["symbolic-ref", "refs/remotes/origin/published"])).toBe(
			"refs/heads/master",
		);
	});

	it("isolates post-push verification from a raced URL rewrite", async () => {
		const parent = await make_root();
		const approved = join(parent, "approved.git");
		const attacker = join(parent, "attacker.git");
		const workspace = join(parent, "workspace");
		const approved_endpoint = pathToFileURL(approved).href;
		const attacker_endpoint = pathToFileURL(attacker).href;
		const rewrite_key = `url.${attacker_endpoint}.insteadOf`;
		let armed = false;
		let injected = false;
		let rewrite_active = false;

		await run_git(parent, ["init", "--bare", "-q", approved]);
		await run_git(parent, ["init", "--bare", "-q", attacker]);
		await fs.mkdir(workspace);
		await initialize_repository(workspace);
		await run_git(workspace, ["remote", "add", "origin", approved_endpoint]);

		const mutation = await make_hooked_mutation(workspace, {
			after: async (input, result) => {
				if (input.args.includes("push") && result.exit_code === 0) {
					armed = true;
				}

				if (rewrite_active && input.args.includes("ls-remote")) {
					await run_git(workspace, ["config", "--local", "--unset-all", rewrite_key]);
					armed = false;
					rewrite_active = false;
				}
			},
			before: async (input) => {
				if (armed && !injected && input.args.includes("ls-remote")) {
					injected = true;
					rewrite_active = true;
					await run_git(workspace, [
						"config",
						"--local",
						"--add",
						rewrite_key,
						approved_endpoint,
					]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({
				remote: "origin",
				set_upstream: false,
				target_branch: "published",
				type: "push",
			}),
		);
		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(injected).toBe(true);
		expect(outcome.type).toBe("applied");
		expect(await read_git(approved, ["for-each-ref", "refs/heads/published"])).not.toBe("");
		expect(await read_git(attacker, ["for-each-ref", "refs/heads/published"])).toBe("");
	});

	it("keeps a checkout settlement race detached and outcome-unknown", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);
		await run_git(root, ["switch", "-qc", "feature"]);
		await fs.writeFile(join(root, "tracked.txt"), "feature\n");
		await run_git(root, ["commit", "-qam", "feature"]);
		const feature_head = await read_git(root, ["rev-parse", "HEAD"]);

		await run_git(root, ["switch", "-q", "master"]);
		const source_head = await read_git(root, ["rev-parse", "HEAD"]);
		const mutation = await make_hooked_mutation(root, {
			after: async (input, result) => {
				if (
					armed &&
					!raced &&
					result.exit_code === 0 &&
					input.args.includes("switch") &&
					input.args.includes("--detach")
				) {
					raced = true;
					await run_git(root, [
						"update-ref",
						"refs/heads/master",
						feature_head,
						source_head,
					]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({ target_branch: "feature", type: "checkout" }),
		);

		armed = true;

		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));
		const worktrees = await read_git(root, ["worktree", "list", "--porcelain"]);

		expect(raced).toBe(true);
		expect(attempt.exit_code).not.toBe(0);
		expect(outcome.type).toBe("outcome_unknown");
		expect(await read_git(root, ["rev-parse", "HEAD"])).toBe(feature_head);
		expect((await run_git(root, ["symbolic-ref", "--quiet", "HEAD"], [1])).exit_code).toBe(1);
		expect(worktrees.match(/^worktree /gmu)).toHaveLength(1);
	});

	it("settles a branch creation race as rejected without changing HEAD", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);

		const source_head = await read_git(root, ["rev-parse", "HEAD"]);
		const mutation = await make_hooked_mutation(root, {
			before: async (input) => {
				if (
					armed &&
					!raced &&
					input.args.includes("update-ref") &&
					input.stdin !== undefined
				) {
					raced = true;
					await run_git(root, ["branch", "feature", source_head]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }),
		);

		armed = true;

		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(raced).toBe(true);
		expect(outcome).toEqual({ reason: "branch_exists", type: "rejected" });
		expect(await read_git(root, ["symbolic-ref", "--short", "HEAD"])).toBe("master");
		expect(await read_git(root, ["rev-parse", "refs/heads/feature"])).toBe(source_head);
	});

	it("leaves a destructive reset race detached for explicit recovery", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);

		const source_head = await read_git(root, ["rev-parse", "HEAD"]);

		await run_git(root, ["switch", "-qc", "target"]);
		await fs.writeFile(join(root, "tracked.txt"), "target\n");
		await run_git(root, ["commit", "-qam", "target"]);
		const target_head = await read_git(root, ["rev-parse", "HEAD"]);

		await run_git(root, ["switch", "-qc", "racer", source_head]);
		await fs.writeFile(join(root, "tracked.txt"), "racer\n");
		await run_git(root, ["commit", "-qam", "racer"]);
		const racer_head = await read_git(root, ["rev-parse", "HEAD"]);

		await run_git(root, ["switch", "-q", "master"]);

		const mutation = await make_hooked_mutation(root, {
			after: async (input, result) => {
				if (armed && !raced && result.exit_code === 0 && input.args.includes("reset")) {
					raced = true;
					await run_git(root, [
						"update-ref",
						"refs/heads/master",
						racer_head,
						source_head,
					]);
				}
			},
		});
		const plan = await Effect.runPromise(
			mutation.Prepare({ mode: "hard", target: target_head, type: "reset" }),
		);

		armed = true;

		const attempt = await Effect.runPromise(mutation.Execute(plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(plan, attempt));

		expect(raced).toBe(true);
		expect(outcome.type).toBe("outcome_unknown");
		expect(await read_git(root, ["rev-parse", "HEAD"])).toBe(target_head);
		expect(await read_git(root, ["rev-parse", "refs/heads/master"])).toBe(racer_head);
		expect(await fs.readFile(join(root, "tracked.txt"), "utf8")).toBe("target\n");
		expect((await run_git(root, ["symbolic-ref", "--quiet", "HEAD"], [1])).exit_code).toBe(1);
	});

	it("preserves a new rebase head raced in after a successful continuation", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);
		await run_git(root, ["switch", "-qc", "feature"]);
		await fs.writeFile(join(root, "tracked.txt"), "feature\n");
		await run_git(root, ["commit", "-qam", "feature"]);
		await run_git(root, ["switch", "-q", "master"]);
		await fs.writeFile(join(root, "tracked.txt"), "master\n");
		await run_git(root, ["commit", "-qam", "master"]);
		const replacement_head = await read_git(root, ["rev-parse", "HEAD"]);
		const git_directory = await read_git(root, ["rev-parse", "--absolute-git-dir"]);

		await run_git(root, ["switch", "-q", "feature"]);

		const mutation = await make_hooked_mutation(root, {
			after: async (input, result) => {
				if (
					armed &&
					!raced &&
					result.exit_code === 0 &&
					input.args.includes("rebase") &&
					input.args.includes("--continue")
				) {
					raced = true;
					await fs.mkdir(join(git_directory, "rebase-merge"), { recursive: true });
					await fs.writeFile(join(git_directory, "REBASE_HEAD"), `${replacement_head}\n`);
				}
			},
		});
		const start_plan = await Effect.runPromise(
			mutation.Prepare({ action: "start", target_branch: "master", type: "rebase" }),
		);
		const start_attempt = await Effect.runPromise(mutation.Execute(start_plan));
		const start_outcome = await Effect.runPromise(
			mutation.Reconcile(start_plan, start_attempt),
		);

		if (start_outcome.type !== "action_required") {
			throw new Error("Expected a rebase conflict");
		}

		await fs.writeFile(join(root, "tracked.txt"), "resolved\n");
		await run_git(root, ["add", "tracked.txt"]);

		const continue_plan = await Effect.runPromise(
			mutation.Prepare({
				action_anchor: start_outcome.anchor,
				operation: { action: "continue", type: "rebase" },
			}),
		);

		armed = true;

		const continued = await Effect.runPromise(mutation.Execute(continue_plan));
		const outcome = await Effect.runPromise(mutation.Reconcile(continue_plan, continued));

		expect(raced).toBe(true);
		expect(outcome.type).toBe("outcome_unknown");
		expect((await fs.readFile(join(git_directory, "REBASE_HEAD"), "utf8")).trim()).toBe(
			replacement_head,
		);
	});

	it("fails closed when a new rebase directory races in before its head", async () => {
		const root = await make_root();
		let armed = false;
		let raced = false;

		await initialize_repository(root);
		await run_git(root, ["switch", "-qc", "feature"]);
		await fs.writeFile(join(root, "tracked.txt"), "feature\n");
		await run_git(root, ["commit", "-qam", "feature"]);
		await run_git(root, ["switch", "-q", "master"]);
		await fs.writeFile(join(root, "tracked.txt"), "master\n");
		await run_git(root, ["commit", "-qam", "master"]);
		const git_directory = await read_git(root, ["rev-parse", "--absolute-git-dir"]);

		await run_git(root, ["switch", "-q", "feature"]);

		const mutation = await make_hooked_mutation(root, {
			after: async (input, result) => {
				if (
					armed &&
					!raced &&
					result.exit_code === 0 &&
					input.args.includes("rebase") &&
					input.args.includes("--continue")
				) {
					raced = true;
					await fs.rm(join(git_directory, "REBASE_HEAD"), { force: true });
					await fs.mkdir(join(git_directory, "rebase-merge"), { recursive: true });
				}
			},
		});
		const start_plan = await Effect.runPromise(
			mutation.Prepare({ action: "start", target_branch: "master", type: "rebase" }),
		);
		const start_attempt = await Effect.runPromise(mutation.Execute(start_plan));
		const start_outcome = await Effect.runPromise(
			mutation.Reconcile(start_plan, start_attempt),
		);

		if (start_outcome.type !== "action_required") {
			throw new Error("Expected a rebase conflict");
		}

		await fs.writeFile(join(root, "tracked.txt"), "resolved\n");
		await run_git(root, ["add", "tracked.txt"]);

		const continue_plan = await Effect.runPromise(
			mutation.Prepare({
				action_anchor: start_outcome.anchor,
				operation: { action: "continue", type: "rebase" },
			}),
		);

		armed = true;

		const continued = await Effect.runPromise(mutation.Execute(continue_plan));
		const outcome = await Effect.runPromise(
			Effect.exit(mutation.Reconcile(continue_plan, continued)),
		);

		expect(raced).toBe(true);
		expect(Exit.isFailure(outcome)).toBe(true);
		expect(
			(await run_git(root, ["show-ref", "--verify", "--quiet", "REBASE_HEAD"], [1]))
				.exit_code,
		).toBe(1);
	});

	it("does not attribute an unrelated merge conflict to a clean plan", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["switch", "-qc", "feature"]);
		await fs.writeFile(join(root, "tracked.txt"), "feature\n");
		await run_git(root, ["commit", "-qam", "feature"]);
		await run_git(root, ["switch", "-q", "master"]);
		await fs.writeFile(join(root, "tracked.txt"), "master\n");
		await run_git(root, ["commit", "-qam", "master"]);
		await fs.writeFile(join(root, "untracked.txt"), "clean candidate\n");

		const mutation = await make_hooked_mutation(root, {});
		const plan = await Effect.runPromise(mutation.Prepare({ type: "clean" }));

		const merge = await run_git(root, ["merge", "feature"], [1]);
		const outcome = await Effect.runPromise(mutation.Reconcile(plan));

		expect(merge.exit_code).toBe(1);
		expect(outcome.type).toBe("outcome_unknown");
	});

	it("never infers applied without the execution receipt", async () => {
		const root = await make_root();

		await initialize_repository(root);

		const mutation = await make_hooked_mutation(root, {});
		const plan = await Effect.runPromise(
			mutation.Prepare({ branch: "feature", type: "branch_create" }),
		);
		const before = await Effect.runPromise(mutation.Reconcile(plan));

		await Effect.runPromise(mutation.Execute(plan));

		const after = await Effect.runPromise(mutation.Reconcile(plan));

		expect(before.type).toBe("source");
		expect(after.type).toBe("outcome_unknown");
	});
});
