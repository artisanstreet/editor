import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { Git, GitError } from "../../modules/backend/src/git/git";
import { GitMutation } from "../../modules/backend/src/git/git-mutation";
import {
	make_git_mutation_layer,
	make_node_git_mutation_layer,
} from "../../modules/backend/src/git/node-git-mutation";
import { make_git_layer, make_node_git_layer } from "../../modules/backend/src/git/node-git";
import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
	type ProcessRunnerShape,
} from "../../modules/backend/src/git/process-runner";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan git mutation "));

	roots.push(root);

	return root;
}

function process_result(
	stdout: string,
	input: ProcessRunnerInput,
	status = 0,
	stderr = "",
): ProcessRunnerResult {
	const stdout_bytes = new TextEncoder().encode(stdout);
	const stderr_bytes = new TextEncoder().encode(stderr);
	const stdout_limit = input.max_stdout_bytes ?? stdout_bytes.byteLength;
	const stderr_limit = input.max_stderr_bytes ?? stderr_bytes.byteLength;

	return {
		exit_code: status,
		stderr: stderr_bytes.slice(0, stderr_limit),
		stderr_bytes: stderr_bytes.byteLength,
		stderr_truncated: stderr_bytes.byteLength > stderr_limit,
		stdout: stdout_bytes.slice(0, stdout_limit),
		stdout_bytes: stdout_bytes.byteLength,
		stdout_truncated: stdout_bytes.byteLength > stdout_limit,
	};
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

async function initialize_repository(root: string, object_format?: "sha1" | "sha256") {
	await run_git(
		root,
		["init", "-q", object_format !== undefined && `--object-format=${object_format}`].filter(
			(argument): argument is string => argument !== false,
		),
	);
	await run_git(root, ["config", "user.email", "test@example.com"]);
	await run_git(root, ["config", "user.name", "Test User"]);
	await fs.writeFile(join(root, "tracked.txt"), "initial\n");
	await run_git(root, ["add", "."]);
	await run_git(root, ["commit", "-qm", "initial"]);
}

async function make_git(root: string) {
	return Effect.runPromise(
		Effect.service(Git).pipe(Effect.provide(make_node_git_layer({ cwd: root }))),
	);
}

async function make_mutation(root: string) {
	return Effect.runPromise(
		Effect.service(GitMutation).pipe(
			Effect.provide(make_node_git_mutation_layer({ cwd: root })),
		),
	);
}

async function make_injected_git(Run: ProcessRunnerShape["Run"]) {
	const runner = Layer.succeed(ProcessRunner, { Run });
	const layer = make_git_layer({ cwd: "injected repository" }).pipe(Layer.provide(runner));

	return Effect.runPromise(Effect.service(Git).pipe(Effect.provide(layer)));
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("Git worktree inventory", () => {
	it("reports one worktree with branch and head metadata", async () => {
		const root = await make_root();

		await initialize_repository(root);

		const git = await make_git(root);
		const worktrees = await Effect.runPromise(git.Worktrees);

		expect(worktrees).toHaveLength(1);
		expect(worktrees[0]).toMatchObject({
			adapter_path: root.replaceAll("\\", "/"),
			bare: false,
			detached: false,
			locked: false,
			prunable: false,
		});
		expect(Option.getOrUndefined(worktrees[0]!.branch)).toBe("refs/heads/master");
		expect(Option.getOrUndefined(worktrees[0]!.head)).toMatch(/^[0-9a-f]{40}$/);
	});

	it("reports multiple worktrees and detached metadata", async () => {
		const root = await make_root();
		const feature_root = await make_root();
		const detached_root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);
		await run_git(root, ["worktree", "add", "-q", feature_root, "feature"]);
		await run_git(root, ["worktree", "add", "-q", "--detach", detached_root, "HEAD"]);
		await run_git(root, ["worktree", "lock", "--reason", "test lock", feature_root]);

		const git = await make_git(root);
		const worktrees = await Effect.runPromise(git.Worktrees);
		const detached = worktrees.find(
			(worktree) => worktree.adapter_path === detached_root.replaceAll("\\", "/"),
		);
		const feature = worktrees.find(
			(worktree) => worktree.adapter_path === feature_root.replaceAll("\\", "/"),
		);

		expect(worktrees).toHaveLength(3);
		expect(detached).toMatchObject({ detached: true, locked: false, prunable: false });
		expect(Option.isNone(detached!.branch)).toBe(true);
		expect(feature).toMatchObject({ detached: false, locked: true, prunable: false });
	});

	it("maps malformed and truncated NUL porcelain to GitError", async () => {
		for (const stdout of [
			"worktree C:/repo\0HEAD deadbeef\0",
			"worktree C:/repo\0HEAD deadbeef\0branch refs/heads/main",
		]) {
			const runner = Layer.succeed(ProcessRunner, {
				Run: (input) => Effect.succeed(process_result(stdout, input)),
			});
			const layer = make_git_layer({ cwd: "injected repository" }).pipe(
				Layer.provide(runner),
			);
			const git = await Effect.runPromise(Effect.service(Git).pipe(Effect.provide(layer)));
			const error = await Effect.runPromise(git.Worktrees.pipe(Effect.flip));

			expect(error).toBeInstanceOf(GitError);
			expect(error.operation).toBe("worktrees");
		}
	});
});

describe("Git coordinator reads", () => {
	it("probes real repositories without failing for an ordinary directory", async () => {
		const repository_root = await make_root();
		const ordinary_root = await make_root();

		await initialize_repository(repository_root);

		const repository_git = await make_git(repository_root);
		const ordinary_git = await make_git(ordinary_root);
		const repository = await Effect.runPromise(repository_git.ProbeRepository);
		const ordinary = await Effect.runPromise(ordinary_git.ProbeRepository);

		expect(Option.getOrUndefined(repository)?.root.replaceAll("/", "\\")).toBe(repository_root);
		expect(Option.isNone(ordinary)).toBe(true);
	});

	it("uses probe exit status without parsing localized stderr", async () => {
		const absent = await make_injected_git((input) =>
			Effect.succeed(process_result("", input, 128, "kein Repository")),
		);
		const unexpected = await make_injected_git((input) =>
			Effect.succeed(process_result("", input, 2, "localized failure")),
		);

		const result = await Effect.runPromise(absent.ProbeRepository);
		const error = await Effect.runPromise(unexpected.ProbeRepository.pipe(Effect.flip));

		expect(Option.isNone(result)).toBe(true);
		expect(error).toBeInstanceOf(GitError);
		expect(error.operation).toBe("probe");
	});

	it("preserves process failures as typed probe errors", async () => {
		const process_error = new ProcessRunnerError({
			cause: new Error("spawn failed"),
			command: "git",
			operation: "spawn",
		});
		const git = await make_injected_git(() => Effect.fail(process_error));

		const error = await Effect.runPromise(git.ProbeRepository.pipe(Effect.flip));

		expect(error).toBeInstanceOf(GitError);
		expect(error.operation).toBe("probe");
		expect(error.cause).toBe(process_error);
	});

	it("resolves exact SHA-1 and SHA-256 local branch object IDs", async () => {
		for (const object_format of ["sha1", "sha256"] as const) {
			const root = await make_root();

			await initialize_repository(root, object_format);
			await run_git(root, ["branch", "feature"]);
			await run_git(root, ["update-ref", "refs/remotes/origin/remote-only", "HEAD"]);

			const git = await make_git(root);
			const repository = await Effect.runPromise(git.Discover);
			const feature = await Effect.runPromise(git.ResolveLocalBranch("feature"));
			const missing = await Effect.runPromise(git.ResolveLocalBranch("missing"));
			const remote_only = await Effect.runPromise(git.ResolveLocalBranch("remote-only"));

			expect(Option.getOrUndefined(feature)).toBe(Option.getOrUndefined(repository.head));
			expect(Option.getOrUndefined(feature)).toMatch(
				object_format === "sha1" ? /^[0-9a-f]{40}$/ : /^[0-9a-f]{64}$/,
			);
			expect(Option.isNone(missing)).toBe(true);
			expect(Option.isNone(remote_only)).toBe(true);
		}
	});

	it("keeps option-looking branch names inside an exact local ref", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const object_id = "a".repeat(64);
		const git = await make_injected_git((input) =>
			Effect.sync(() => {
				calls.push(input);

				return input.args.includes("--exists")
					? process_result("", input)
					: process_result(`${object_id}\n`, input);
			}),
		);

		const resolved = await Effect.runPromise(git.ResolveLocalBranch("--looks-like-an-option"));

		expect(Option.getOrUndefined(resolved)).toBe(object_id);
		expect(calls.map((call) => call.args)).toEqual([
			["show-ref", "--exists", "--", "refs/heads/--looks-like-an-option"],
			["show-ref", "--verify", "--hash", "--", "refs/heads/--looks-like-an-option"],
		]);
	});

	it("rejects invalid branch inputs before process execution", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const git = await make_injected_git((input) =>
			Effect.sync(() => {
				calls.push(input);

				return process_result("", input, 2);
			}),
		);

		const empty = await Effect.runPromise(git.ResolveLocalBranch("   ").pipe(Effect.flip));
		const nul = await Effect.runPromise(
			git.ResolveLocalBranch("feature\0suffix").pipe(Effect.flip),
		);

		expect(empty.operation).toBe("resolve_branch");
		expect(nul.operation).toBe("resolve_branch");
		expect(calls).toHaveLength(0);
	});
});

describe("GitMutation", () => {
	it("checks out an existing local branch", async () => {
		const root = await make_root();

		await initialize_repository(root);
		await run_git(root, ["branch", "feature"]);

		const mutation = await make_mutation(root);
		await Effect.runPromise(mutation.CheckoutLocalBranch("feature"));

		const branch = await make_git(root).then((git) => Effect.runPromise(git.Discover));
		expect(branch.branch).toBe("feature");
	});

	it("maps nonexistent branches and rejects invalid input", async () => {
		const root = await make_root();

		await initialize_repository(root);

		const mutation = await make_mutation(root);
		const missing = await Effect.runPromise(
			mutation.CheckoutLocalBranch("missing").pipe(Effect.flip),
		);
		const empty = await Effect.runPromise(
			mutation.CheckoutLocalBranch("   ").pipe(Effect.flip),
		);
		const nul = await Effect.runPromise(
			mutation.CheckoutLocalBranch("feature\0x").pipe(Effect.flip),
		);

		expect(missing._tag).toBe("GitMutationError");
		expect(missing.operation).toBe("checkout");
		expect(empty._tag).toBe("GitMutationError");
		expect(nul._tag).toBe("GitMutationError");
	});

	it("passes option-looking branches after the argv option terminator", async () => {
		const calls: Array<ProcessRunnerInput> = [];
		const runner = Layer.succeed(ProcessRunner, {
			Run: (input) => {
				calls.push(input);

				return Effect.succeed(process_result("", input));
			},
		});
		const layer = make_git_mutation_layer({ cwd: "injected repository" }).pipe(
			Layer.provide(runner),
		);
		const mutation = await Effect.runPromise(
			Effect.service(GitMutation).pipe(Effect.provide(layer)),
		);

		await Effect.runPromise(mutation.CheckoutLocalBranch("--looks-like-an-option"));

		expect(calls).toHaveLength(1);
		expect(calls[0]!.args).toEqual(["switch", "--no-guess", "--", "--looks-like-an-option"]);
		expect(calls[0]!.max_stdout_bytes).toBeGreaterThan(0);
		expect(calls[0]!.max_stderr_bytes).toBeGreaterThan(0);
		expect(calls.some((call) => call.args.includes("worktree"))).toBe(false);
	});
});
