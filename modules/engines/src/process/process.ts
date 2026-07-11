import { type ChildProcess, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";

import cross_spawn from "cross-spawn";

import { Context, Effect, Layer } from "effect";

import { EngineProcessError } from "../engine";
import {
	open_windows_job_candidate,
	type WindowsJob,
	type WindowsJobCandidate,
} from "./windows-job";

/** Describes the terminal state reported by a spawned child process. @since 0.4.0 */
export interface EngineProcessExit {
	readonly code: number | null;
	readonly signal: NodeJS.Signals | null;
}

/** Supplies the executable and environment used to spawn one engine process. @since 0.4.0 */
export interface EngineProcessSpawnInput {
	readonly args: ReadonlyArray<string>;
	readonly command: string;
	readonly cwd?: string;
	readonly env?: NodeJS.ProcessEnv;
}

/** Owns byte streams and lifecycle controls for one spawned engine process. @since 0.4.0 */
export interface EngineProcessHandle {
	readonly Exit: Effect.Effect<EngineProcessExit, EngineProcessError>;
	readonly Kill: (signal?: NodeJS.Signals) => Effect.Effect<void, EngineProcessError>;
	readonly Close: Effect.Effect<void>;
	readonly EndInput: Effect.Effect<void, EngineProcessError>;
	readonly Stderr: AsyncIterable<Uint8Array>;
	readonly Stdout: AsyncIterable<Uint8Array>;
	readonly Write: (chunk: Uint8Array) => Effect.Effect<void, EngineProcessError>;
}

/** Spawns and owns provider-neutral engine CLI process handles. @since 0.4.0 */
export class EngineProcessFactory extends Context.Service<
	EngineProcessFactory,
	{
		readonly Spawn: (
			input: EngineProcessSpawnInput,
		) => Effect.Effect<EngineProcessHandle, EngineProcessError>;
	}
>()("Artisan/EngineProcessFactory") {}

function make_process_exit(
	child: ChildProcessWithoutNullStreams,
	release: () => void = () => undefined,
) {
	let process_closed = false;
	let settled = false;
	let fail_exit = (_cause: unknown) => undefined;
	const exit = new Promise<EngineProcessExit>((resolve, reject) => {
		fail_exit = (cause) => {
			if (settled) {
				return;
			}

			settled = true;
			release();
			reject(cause);
		};

		child.once("error", fail_exit);
		child.once("close", (code, signal) => {
			process_closed = true;

			if (settled) {
				return;
			}

			settled = true;
			release();
			resolve({ code, signal });
		});
	});

	exit.catch(() => undefined);

	return {
		Exit: Effect.tryPromise({
			try: () => exit,
			catch: (cause) => new EngineProcessError({ cause, operation: "exit" }),
		}),
		Fail: fail_exit,
		IsClosed: () => process_closed,
	};
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
	return Effect.try({
		try: () => {
			if (child.stdin.destroyed || child.stdin.writableEnded) {
				return;
			}

			child.stdin.once("error", () => undefined);
			child.stdin.end();
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "close" }),
	});
}

function is_no_such_process(cause: unknown) {
	return cause instanceof Error && "code" in cause && cause.code === "ESRCH";
}

function make_posix_tree_kill(child: ChildProcessWithoutNullStreams, signal: NodeJS.Signals) {
	if (child.pid === undefined) {
		return Effect.void;
	}

	if (!is_process_running(child)) {
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

interface WindowsProcessHostMessage {
	readonly process_id: number;
	readonly type: "ready";
}

interface WindowsProcessHostClaim {
	readonly claim_token: string;
	readonly type: "claim_ack";
}

interface WindowsProcessHostError {
	readonly message: string;
	readonly type: "spawn_error";
}

interface OwnedEngineProcess {
	readonly child: ChildProcessWithoutNullStreams;
	readonly process_exit: ReturnType<typeof make_process_exit>;
	readonly Terminate: (signal: NodeJS.Signals) => Effect.Effect<void, EngineProcessError>;
}

type WindowsProcessHost = ChildProcessWithoutNullStreams & {
	readonly send: NonNullable<ChildProcess["send"]>;
};

const windows_process_host_path = fileURLToPath(
	new URL("./windows-process-host.mjs", import.meta.url),
);

function is_windows_process_host_message(
	message: unknown,
): message is WindowsProcessHostMessage | WindowsProcessHostError | WindowsProcessHostClaim {
	return (
		message !== null &&
		typeof message === "object" &&
		"type" in message &&
		(message.type === "ready" || message.type === "spawn_error" || message.type === "claim_ack")
	);
}

function spawn_windows_process(input: EngineProcessSpawnInput) {
	return new Promise<OwnedEngineProcess>((resolve, reject) => {
		const spawned_child = cross_spawn(process.execPath, [windows_process_host_path], {
			detached: true,
			env: {
				...process.env,
				...(process.versions.electron ? { ELECTRON_RUN_AS_NODE: "1" } : {}),
			},
			stdio: ["pipe", "pipe", "pipe", "ipc"],
			windowsHide: true,
		});
		const claim_token = randomUUID();
		let candidate: WindowsJobCandidate | undefined;
		let job: WindowsJob | undefined;
		let settled = false;

		if (
			!spawned_child.stdin ||
			!spawned_child.stderr ||
			!spawned_child.stdout ||
			spawned_child.send === undefined
		) {
			spawned_child.kill("SIGKILL");
			reject(new Error("Windows process host did not provide piped IPC and stdio"));

			return;
		}

		const child = spawned_child as WindowsProcessHost;
		const release = () => {
			candidate?.Close();
			job?.Close();
		};
		const process_exit = make_process_exit(child, release);
		const timeout = setTimeout(() => {
			fail(new Error("Windows process host startup timed out"));
		}, 5_000);
		const fail = (cause: unknown) => {
			if (settled) {
				return;
			}

			settled = true;
			clearTimeout(timeout);

			try {
				if (job) {
					job.Terminate(1);
				} else {
					child.kill("SIGKILL");
				}
			} catch {
				child.kill("SIGKILL");
			}

			release();
			reject(cause);
		};

		child.once("spawn", () => {
			if (child.pid === undefined) {
				fail(new Error("Windows process host did not expose a PID"));

				return;
			}

			try {
				candidate = open_windows_job_candidate(child.pid);
			} catch (cause) {
				fail(cause);

				return;
			}

			child.send({ claim_token, type: "claim" }, (cause) => {
				if (cause) {
					fail(cause);
				}
			});
		});
		child.on("message", (message) => {
			if (!is_windows_process_host_message(message)) {
				return;
			}

			if (message.type === "spawn_error") {
				const cause = new Error(message.message);

				if (settled) {
					process_exit.Fail(cause);
				} else {
					fail(cause);
				}

				return;
			}

			if (settled) {
				return;
			}

			if (message.type === "claim_ack") {
				if (message.claim_token !== claim_token || !candidate) {
					fail(new Error("Windows process host returned an invalid claim proof"));

					return;
				}

				try {
					job = candidate.Assign();
				} catch (cause) {
					fail(cause);

					return;
				}

				child.send({ input, type: "start" }, (cause) => {
					if (cause) {
						fail(cause);
					}
				});

				return;
			}

			if (!Number.isSafeInteger(message.process_id) || message.process_id <= 0 || !job) {
				fail(new Error("Windows process host returned an invalid ready message"));

				return;
			}

			const owned_job = job;

			settled = true;
			clearTimeout(timeout);
			resolve({
				child,
				process_exit,
				Terminate: () =>
					Effect.try({
						try: () => owned_job.Terminate(1),
						catch: (cause) => new EngineProcessError({ cause, operation: "kill" }),
					}),
			});
		});
		child.once("error", fail);
		child.once("close", () => fail(new Error("Windows process host exited during startup")));
	});
}

function spawn_posix_process(input: EngineProcessSpawnInput): OwnedEngineProcess {
	const spawned_child = cross_spawn(input.command, input.args, {
		cwd: input.cwd,
		detached: true,
		env: input.env,
		shell: false,
		stdio: "pipe",
		windowsHide: true,
	});

	if (!spawned_child.stdin || !spawned_child.stderr || !spawned_child.stdout) {
		throw new Error("Engine process did not provide piped stdio");
	}

	const child = spawned_child as ChildProcessWithoutNullStreams;

	return {
		child,
		process_exit: make_process_exit(child),
		Terminate: (signal) => make_posix_tree_kill(child, signal),
	};
}

function spawn_owned_process(input: EngineProcessSpawnInput) {
	return process.platform === "win32"
		? spawn_windows_process(input)
		: Promise.resolve(spawn_posix_process(input));
}

/** Provides the Node child-process implementation used by all CLI engines. @since 0.4.0 */
export const EngineProcessFactoryLive = Layer.succeed(EngineProcessFactory, {
	Spawn: (input) =>
		Effect.tryPromise({
			try: () => spawn_owned_process(input),
			catch: (cause) => new EngineProcessError({ cause, operation: "spawn" }),
		}).pipe(
			Effect.map((owned) => {
				const child = owned.child;
				const process_exit = owned.process_exit;
				const Exit = process_exit.Exit;
				const Kill = (signal: NodeJS.Signals = "SIGTERM") =>
					process_exit.IsClosed() ? Effect.void : owned.Terminate(signal);
				const EndInput = make_close_stdin(child);
				let close_started = false;
				const Close: Effect.Effect<void> = Effect.suspend(() => {
					if (process_exit.IsClosed()) {
						return Effect.void;
					}

					if (close_started) {
						return Exit.pipe(Effect.timeoutOption(500), Effect.ignore);
					}

					close_started = true;

					return Effect.gen(function* () {
						if (process.platform === "win32") {
							yield* Kill("SIGTERM").pipe(Effect.ignore);
						} else {
							yield* EndInput.pipe(Effect.ignore);

							if (is_process_running(child)) {
								yield* Kill("SIGTERM").pipe(Effect.ignore);
							}
						}

						yield* Exit.pipe(Effect.timeoutOption(750), Effect.ignore);

						if (!process_exit.IsClosed()) {
							yield* Kill("SIGKILL").pipe(Effect.ignore);
						}

						yield* Exit.pipe(Effect.timeoutOption(750), Effect.ignore);
					});
				});

				return {
					Close,
					EndInput,
					Exit,
					Kill,
					Stderr: child.stderr,
					Stdout: child.stdout,
					Write: (chunk: Uint8Array) => make_write(child, chunk),
				};
			}),
		),
});
