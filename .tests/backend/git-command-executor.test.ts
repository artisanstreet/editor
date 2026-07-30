import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";

import { Effect, Exit, Layer, Sink, Stream } from "effect";
import {
	ChildProcessSpawner,
	make as make_spawner,
	makeHandle,
	ProcessId,
} from "effect/unstable/process/ChildProcessSpawner";
import { afterEach, describe, expect, it } from "vitest";

import {
	GitCommandExecutor,
	make_git_command_executor_layer,
	make_node_git_command_executor_layer,
} from "../../modules/backend/src/git/executor";

const roots: Array<string> = [];

async function make_root() {
	const root = await fs.mkdtemp(`${tmpdir()}/artisan-effect-git-command-`);

	roots.push(root);

	return root;
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("GitCommandExecutor", () => {
	it("concurrently drains both streams while retaining bounded output", async () => {
		const root = await make_root();
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args: ["help", "-a"],
					cwd: root,
					max_stderr_bytes: 8,
					max_stdin_bytes: 0,
					max_stdout_bytes: 32,
					mode: "read",
				});
			}).pipe(Effect.provide(make_node_git_command_executor_layer())),
		);

		expect(result.exit_code).toBe(-1);
		expect(result.termination).toBe("output_limit");
		expect(result.output_limit_channel).toBe("stdout");
		expect(result.stdout.bytes.byteLength).toBe(32);
		expect(result.stdout.total_bytes).toBeGreaterThan(32);
		expect(result.stdout.truncated).toBe(true);
	});

	it("does not inherit Git environment variables that can redirect repository access", async () => {
		const root = await make_root();
		const layer = make_node_git_command_executor_layer();
		const Run = (args: ReadonlyArray<string>) =>
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args,
					cwd: root,
					max_stderr_bytes: 1024,
					max_stdin_bytes: 0,
					max_stdout_bytes: 1024,
					mode: "read",
				});
			}).pipe(Effect.provide(layer));

		await Effect.runPromise(Run(["init", "-q"]));
		const previous_git_dir = process.env.GIT_DIR;
		const previous_git_work_tree = process.env.GIT_WORK_TREE;
		process.env.GIT_DIR = `${root}/redirected.git`;
		process.env.GIT_WORK_TREE = `${root}/redirected-worktree`;

		try {
			const result = await Effect.runPromise(Run(["rev-parse", "--git-dir"]));

			expect(result.exit_code).toBe(0);
			expect(new TextDecoder().decode(result.stdout.bytes).trim()).toBe(".git");
		} finally {
			if (previous_git_dir === undefined) delete process.env.GIT_DIR;
			else process.env.GIT_DIR = previous_git_dir;
			if (previous_git_work_tree === undefined) delete process.env.GIT_WORK_TREE;
			else process.env.GIT_WORK_TREE = previous_git_work_tree;
		}
	});

	it("stops an unbounded output stream at the configured byte cap", async () => {
		let finalized = false;
		const handle = makeHandle({
			all: Stream.never,
			exitCode: Effect.never,
			getInputFd: () => Sink.drain,
			getOutputFd: () => Stream.empty,
			isRunning: Effect.succeed(true),
			kill: () => Effect.void,
			pid: ProcessId(124),
			stderr: Stream.never,
			stdin: Sink.drain,
			stdout: Stream.make(new Uint8Array(4_096)),
			unref: Effect.succeed(Effect.void),
		});
		const spawner = make_spawner(() =>
			Effect.acquireRelease(Effect.succeed(handle), () =>
				Effect.sync(() => void (finalized = true)),
			),
		);
		const layer = make_git_command_executor_layer().pipe(
			Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
		);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args: ["status"],
					cwd: "C:/repository",
					max_stderr_bytes: 1,
					max_stdin_bytes: 0,
					max_stdout_bytes: 16,
					mode: "read",
				});
			}).pipe(Effect.provide(layer), Effect.timeout("1 second")),
		);

		expect(result.termination).toBe("output_limit");
		expect(result.output_limit_channel).toBe("stdout");
		expect(result.stdout.bytes).toHaveLength(16);
		expect(finalized).toBe(true);
	});

	it("stops a silent process at the executor deadline", async () => {
		let finalized = false;
		const handle = makeHandle({
			all: Stream.never,
			exitCode: Effect.never,
			getInputFd: () => Sink.drain,
			getOutputFd: () => Stream.empty,
			isRunning: Effect.succeed(true),
			kill: () => Effect.void,
			pid: ProcessId(125),
			stderr: Stream.never,
			stdin: Sink.drain,
			stdout: Stream.never,
			unref: Effect.succeed(Effect.void),
		});
		const spawner = make_spawner(() =>
			Effect.acquireRelease(Effect.succeed(handle), () =>
				Effect.sync(() => void (finalized = true)),
			),
		);
		const layer = make_git_command_executor_layer({ timeout: "25 millis" }).pipe(
			Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
		);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args: ["status"],
					cwd: "C:/repository",
					max_stderr_bytes: 1,
					max_stdin_bytes: 0,
					max_stdout_bytes: 1,
					mode: "read",
				});
			}).pipe(Effect.provide(layer), Effect.timeout("1 second")),
		);

		expect(result.termination).toBe("timeout");
		expect(finalized).toBe(true);
	});

	it("writes bounded binary stdin without using a shell", async () => {
		const root = await make_root();
		const stdin = new TextEncoder().encode("content with spaces\n");
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args: ["hash-object", "--stdin"],
					cwd: root,
					max_stderr_bytes: 1024,
					max_stdin_bytes: stdin.byteLength,
					max_stdout_bytes: 1024,
					mode: "mutation",
					stdin,
				});
			}).pipe(Effect.provide(make_node_git_command_executor_layer())),
		);

		expect(result.exit_code).toBe(0);
		expect(new TextDecoder().decode(result.stdout.bytes).trim()).toMatch(/^[a-f0-9]{40}$/u);
	});

	it("interrupts the scoped process handle when command execution is cancelled", async () => {
		let finalized = false;
		const handle = makeHandle({
			all: Stream.never,
			exitCode: Effect.never,
			getInputFd: () => Sink.drain,
			getOutputFd: () => Stream.empty,
			isRunning: Effect.succeed(true),
			kill: () => Effect.void,
			pid: ProcessId(123),
			stderr: Stream.never,
			stdin: Sink.drain,
			stdout: Stream.never,
			unref: Effect.succeed(Effect.void),
		});
		const spawner = make_spawner(() =>
			Effect.acquireRelease(Effect.succeed(handle), () =>
				Effect.sync(() => void (finalized = true)),
			),
		);
		const layer = make_git_command_executor_layer().pipe(
			Layer.provide(Layer.succeed(ChildProcessSpawner, spawner)),
		);
		const result = await Effect.runPromiseExit(
			Effect.gen(function* () {
				const executor = yield* GitCommandExecutor;

				return yield* executor.Run({
					args: ["status"],
					cwd: "C:/repository",
					max_stderr_bytes: 1,
					max_stdin_bytes: 0,
					max_stdout_bytes: 1,
					mode: "read",
				});
			}).pipe(Effect.timeout("25 millis"), Effect.provide(layer)),
		);

		expect(Exit.isFailure(result)).toBe(true);
		expect(finalized).toBe(true);
	});
});
