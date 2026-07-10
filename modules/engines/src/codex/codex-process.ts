import { execFile, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";

import { Context, Effect, Exit as EffectExit, Layer, Option } from "effect";

import { EngineProcessError } from "../engine";

/** Describes the terminal state reported by a spawned child process. @since 0.1.0 */
export interface CodexProcessExit {
	readonly code: number | null;
	readonly signal: NodeJS.Signals | null;
}

/** Supplies the executable and environment used to spawn a Codex process. @since 0.1.0 */
export interface CodexProcessSpawnInput {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
	readonly cwd?: string;
	readonly env?: NodeJS.ProcessEnv;
	readonly shell?: boolean;
}

/** Owns the byte streams and lifecycle controls for one spawned Codex process. @since 0.1.0 */
export interface CodexProcessHandle {
	readonly Exit: Effect.Effect<CodexProcessExit, EngineProcessError>;
	readonly Kill: (signal?: NodeJS.Signals) => Effect.Effect<void, EngineProcessError>;
	readonly Close: Effect.Effect<void>;
	readonly Stderr: AsyncIterable<Uint8Array>;
	readonly Stdout: AsyncIterable<Uint8Array>;
	readonly Write: (chunk: Uint8Array) => Effect.Effect<void, EngineProcessError>;
}

/** Spawns and owns Codex CLI process handles. @since 0.1.0 */
export class CodexProcessFactory extends Context.Service<
	CodexProcessFactory,
	{
		readonly Spawn: (
			input: CodexProcessSpawnInput,
		) => Effect.Effect<CodexProcessHandle, EngineProcessError>;
	}
>()("Artisan/CodexProcessFactory") {}

function make_process_exit(child: ChildProcessWithoutNullStreams) {
	const exit = new Promise<CodexProcessExit>((resolve, reject) => {
		child.once("error", reject);
		child.once("close", (code, signal) => resolve({ code, signal }));
	});

	return Effect.tryPromise({
		try: () => exit,
		catch: (cause) => new EngineProcessError({ cause, operation: "exit" }),
	});
}

function make_write(child: ChildProcessWithoutNullStreams, chunk: Uint8Array) {
	return Effect.tryPromise({
		try: () =>
			new Promise<void>((resolve, reject) => {
				child.stdin.write(chunk, (error) => (error ? reject(error) : resolve()));
			}),
		catch: (cause) => new EngineProcessError({ cause, operation: "write" }),
	});
}

function is_process_running(child: ChildProcessWithoutNullStreams) {
	return child.exitCode === null && child.signalCode === null;
}

function make_close_stdin(child: ChildProcessWithoutNullStreams) {
	return Effect.tryPromise({
		try: () =>
			new Promise<void>((resolve, reject) => {
				if (child.stdin.destroyed || child.stdin.writableEnded) {
					resolve();

					return;
				}

				child.stdin.end((error: Error | null | undefined) =>
					error ? reject(error) : resolve(),
				);
			}),
		catch: (cause) => new EngineProcessError({ cause, operation: "close" }),
	});
}

function is_no_such_process(cause: unknown) {
	return cause instanceof Error && "code" in cause && cause.code === "ESRCH";
}

function read_windows_child_pids(parent_pid: number) {
	return new Promise<ReadonlyArray<number>>((resolve, reject) => {
		execFile(
			"powershell.exe",
			[
				"-NoProfile",
				"-NonInteractive",
				"-Command",
				`(Get-CimInstance Win32_Process -Filter 'ParentProcessId = ${parent_pid}').ProcessId -join ','`,
			],
			{ windowsHide: true },
			(error, stdout) => {
				if (error) {
					reject(error);

					return;
				}

				const pids = stdout
					.trim()
					.split(",")
					.filter(Boolean)
					.map(Number)
					.filter(Number.isInteger);

				resolve(pids);
			},
		);
	});
}

function taskkill_tree(pid: number) {
	return new Promise<void>((resolve) => {
		execFile("taskkill", ["/pid", String(pid), "/t", "/f"], { windowsHide: true }, () =>
			resolve(),
		);
	});
}

function make_windows_tree_kill(
	child: ChildProcessWithoutNullStreams,
	signal: NodeJS.Signals,
	known_child_pids?: ReadonlyArray<number> | null,
) {
	if (!is_process_running(child) || child.pid === undefined) {
		return Effect.void;
	}
	const pid = child.pid;

	return Effect.tryPromise({
		try: async () => {
			let child_pids: ReadonlyArray<number>;

			if (known_child_pids === null) {
				await taskkill_tree(pid);

				return;
			}

			if (known_child_pids) {
				child_pids = known_child_pids;
			} else {
				try {
					child_pids = await read_windows_child_pids(pid);
				} catch {
					await taskkill_tree(pid);

					return;
				}
			}

			await Promise.all(child_pids.map(taskkill_tree));

			if (is_process_running(child)) {
				child.kill(signal);
			}
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "kill" }),
	});
}

function make_posix_tree_kill(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals) {
	if (!is_process_running(child) || child.pid === undefined) {
		return Effect.void;
	}
	const pid = child.pid;

	return Effect.try({
		try: () => {
			try {
				process.kill(-pid, signal);
			} catch (cause) {
				if (!is_no_such_process(cause)) {
					throw cause;
				}
			}
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "kill" }),
	});
}

function make_tree_kill(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals) {
	return process.platform === "win32"
		? make_windows_tree_kill(child, signal)
		: make_posix_tree_kill(child, signal);
}

/** Provides a Node child-process implementation of the Codex process seam. @since 0.1.0 */
export const CodexProcessFactoryLive = Layer.succeed(CodexProcessFactory, {
	Spawn: (input) =>
		Effect.try({
			try: () => {
				const child = spawn(input.command, input.args, {
					cwd: input.cwd,
					detached: process.platform !== "win32",
					env: input.env,
					shell: input.shell,
					stdio: "pipe",
				});
				const Exit = make_process_exit(child);
				const Kill = (signal: NodeJS.Signals = "SIGTERM") => make_tree_kill(child, signal);
				let close_started = false;
				const Close = Effect.suspend(() => {
					if (close_started) {
						return Exit.pipe(Effect.timeoutOption(500), Effect.ignore);
					}

					close_started = true;

					return Effect.gen(function* () {
						const child_pid = child.pid;
						const child_pids =
							process.platform === "win32" && child_pid !== undefined
								? yield* Effect.tryPromise({
										try: () => read_windows_child_pids(child_pid),
										catch: (cause) =>
											new EngineProcessError({ cause, operation: "kill" }),
									}).pipe(Effect.timeoutOption(1_000), Effect.exit)
								: undefined;
						const known_child_pids =
							child_pids &&
							EffectExit.isSuccess(child_pids) &&
							Option.isSome(child_pids.value)
								? child_pids.value.value
								: null;

						yield* make_close_stdin(child).pipe(Effect.ignore);

						if (is_process_running(child)) {
							yield* (
								process.platform === "win32"
									? make_windows_tree_kill(child, "SIGTERM", known_child_pids)
									: Kill("SIGTERM")
							).pipe(Effect.ignore);
						}

						yield* Exit.pipe(Effect.timeoutOption(250), Effect.ignore);

						if (is_process_running(child)) {
							yield* Kill("SIGKILL").pipe(Effect.ignore);
						}

						yield* Exit.pipe(Effect.timeoutOption(500), Effect.ignore);
					});
				});

				return {
					Close,
					Exit,
					Kill,
					Stderr: child.stderr,
					Stdout: child.stdout,
					Write: (chunk) => make_write(child, chunk),
				};
			},
			catch: (cause) => new EngineProcessError({ cause, operation: "spawn" }),
		}),
});
