import { type ChildProcess, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { statSync } from "node:fs";
import { isSea } from "node:sea";
import { fileURLToPath } from "node:url";

import * as NodeFileSystem from "@effect/platform-node-shared/NodeFileSystem";
import cross_spawn from "cross-spawn";

import { Context, Deferred, Effect, FileSystem, Layer, Scope } from "effect";

import { EngineProcessError } from "../engine";
import {
	open_windows_job_candidate,
	type WindowsJob,
	type WindowsJobCandidate,
} from "./windows-job";
import { WindowsProcessHostModeArgument } from "./windows-process-host-mode";

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
	/**
	 * Selects the account home this process runs against. Absent means the
	 * engine's default profile, so callers that predate multiple accounts keep
	 * their behaviour. Consumed by the spawn override, not by the OS spawn.
	 * @since 0.5.0
	 */
	readonly profile_id?: string;
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
		) => Effect.Effect<EngineProcessHandle, EngineProcessError, Scope.Scope>;
	}
>()("Artisan/EngineProcessFactory") {}

export interface EngineProcessEnvironmentService {
	readonly environment: NodeJS.ProcessEnv;
	readonly is_electron: boolean;
	readonly node_executable: string;
	readonly platform: NodeJS.Platform;
	readonly MakeClaimToken: Effect.Effect<string>;
	readonly windows_process_host_args: ReadonlyArray<string>;
	readonly windows_process_host_path: string;
}

export class EngineProcessEnvironment extends Context.Service<
	EngineProcessEnvironment,
	EngineProcessEnvironmentService
>()("Artisan/EngineProcessEnvironment") {}

export interface EngineProcessRuntime {
	readonly environment: NodeJS.ProcessEnv;
	readonly exec_path: string;
	readonly is_electron: boolean;
	readonly is_sea: boolean;
	readonly platform: NodeJS.Platform;
}

/** Resolves the process host configuration from an explicitly supplied runtime snapshot. */
export const MakeEngineProcessEnvironmentLayer = (runtime: EngineProcessRuntime) =>
	Layer.effect(
		EngineProcessEnvironment,
		Effect.gen(function* () {
			if (runtime.is_sea) {
				return {
					environment: { ...runtime.environment },
					is_electron: runtime.is_electron,
					node_executable: runtime.exec_path,
					platform: runtime.platform,
					MakeClaimToken: Effect.sync(randomUUID),
					windows_process_host_args: [WindowsProcessHostModeArgument],
					windows_process_host_path: runtime.exec_path,
				};
			}

			const file_system = yield* FileSystem.FileSystem;
			const configured_path = runtime.environment.ARTISAN_WINDOWS_PROCESS_HOST?.trim();
			const built_path = fileURLToPath(new URL("./windows-process-host.js", import.meta.url));
			const source_path = fileURLToPath(
				new URL("./windows-process-host-entry.ts", import.meta.url),
			);
			const built_exists =
				configured_path === undefined || configured_path.length === 0
					? yield* file_system.access(built_path).pipe(
							Effect.as(true),
							Effect.catch(() => Effect.succeed(false)),
						)
					: false;

			const windows_process_host_path =
				configured_path && configured_path.length > 0
					? configured_path
					: built_exists
						? built_path
						: source_path;

			return {
				environment: { ...runtime.environment },
				is_electron: runtime.is_electron,
				node_executable: runtime.environment.ARTISAN_NODE_EXECUTABLE ?? runtime.exec_path,
				platform: runtime.platform,
				MakeClaimToken: Effect.sync(randomUUID),
				windows_process_host_args: [windows_process_host_path],
				windows_process_host_path,
			};
		}),
	);

/** Captures Node process configuration once at the executable composition boundary. */
export const EngineProcessEnvironmentLive = Layer.unwrap(
	Effect.sync(() =>
		MakeEngineProcessEnvironmentLayer({
			environment: { ...process.env },
			exec_path: process.execPath,
			is_electron: process.versions.electron !== undefined,
			is_sea: isSea(),
			platform: process.platform,
		}),
	),
).pipe(Layer.provide(NodeFileSystem.layer));

function make_process_exit(
	child: ChildProcessWithoutNullStreams,
	release: () => void = () => undefined,
) {
	const exit = Deferred.makeUnsafe<EngineProcessExit, EngineProcessError>();
	let process_closed = false;
	const fail_exit = (cause: unknown) => {
		const completed = Deferred.doneUnsafe(
			exit,
			Effect.fail(new EngineProcessError({ cause, operation: "exit" })),
		);

		if (completed) {
			release();
		}
	};
	const close_exit = (code: number | null, signal: NodeJS.Signals | null) => {
		process_closed = true;

		if (Deferred.doneUnsafe(exit, Effect.succeed({ code, signal }))) {
			release();
		}
	};

	child.once("error", fail_exit);
	child.once("close", close_exit);

	return {
		Cleanup: Effect.sync(() => {
			child.off("error", fail_exit);
			child.off("close", close_exit);
			release();
		}),
		Exit: Deferred.await(exit),
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

function SpawnWindowsProcess(
	input: EngineProcessSpawnInput,
	environment: EngineProcessEnvironmentService,
	claim_token: string,
) {
	return Effect.callback<OwnedEngineProcess, EngineProcessError>((resume) => {
		const spawned_child = cross_spawn(
			environment.node_executable,
			[...environment.windows_process_host_args],
			{
				detached: true,
				env: {
					...environment.environment,
					...(environment.is_electron ? { ELECTRON_RUN_AS_NODE: "1" } : {}),
				},
				stdio: ["pipe", "pipe", "pipe", "ipc"],
				windowsHide: true,
			},
		);
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
			resume(
				Effect.fail(
					new EngineProcessError({
						cause: new Error(
							"Windows process host did not provide piped IPC and stdio",
						),
						operation: "spawn",
					}),
				),
			);

			return Effect.void;
		}

		const child = spawned_child as WindowsProcessHost;
		const release = () => {
			candidate?.Close();
			job?.Close();
		};
		const process_exit = make_process_exit(child, release);
		const fail = (cause: unknown) => {
			if (settled) {
				return;
			}

			settled = true;

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
			resume(
				Effect.fail(
					new EngineProcessError({
						cause,
						operation: "spawn",
					}),
				),
			);
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
			resume(
				Effect.succeed({
					child,
					process_exit,
					Terminate: () =>
						Effect.try({
							try: () => owned_job.Terminate(1),
							catch: (cause) => new EngineProcessError({ cause, operation: "kill" }),
						}),
				}),
			);
		});
		child.once("error", fail);
		child.once("close", () => fail(new Error("Windows process host exited during startup")));

		return Effect.sync(() => {
			if (!settled) {
				fail(new Error("Windows process host startup was interrupted"));
			}
		});
	}).pipe(
		Effect.timeoutOrElse({
			duration: 5_000,
			orElse: () =>
				Effect.fail(
					new EngineProcessError({
						cause: new Error("Windows process host startup timed out"),
						operation: "spawn",
					}),
				),
		}),
	);
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

/**
 * Stable public error identifier for a spawn rejected before any process ran.
 * A wire-level value rather than a catalog import: engines classifies native
 * failures before any catalog or presentation layer exists, and the catalog
 * owns the human-readable definition for the same stable value.
 */
export const engine_process_working_directory_missing_code = "AE-CLIENT_STATE-107";

/**
 * Refuses a spawn whose working directory does not exist, as its own
 * classified failure. The OS reports that spawn as an opaque instant death —
 * on Windows the process host fails before the CLI even runs — which settles
 * into the transcript as a generic startup failure the user retries forever.
 * Naming the actual fault turns that loop into one actionable card: the
 * folder is gone; restore it or repoint the thread.
 */
const EnsureSpawnWorkingDirectory = (input: EngineProcessSpawnInput) =>
	Effect.gen(function* () {
		if (input.cwd === undefined) return;
		const cwd = input.cwd;
		const exists = yield* Effect.sync(() => {
			try {
				return statSync(cwd).isDirectory();
			} catch {
				return false;
			}
		});
		if (!exists) {
			return yield* Effect.fail(
				new EngineProcessError({
					artisan_code: engine_process_working_directory_missing_code,
					cause: new Error(`Working directory does not exist: ${cwd}`),
					operation: "spawn",
				}),
			);
		}
	});

/** Provides the Node child-process implementation used by all CLI engines. @since 0.4.0 */
export const EngineProcessFactoryLayer = Layer.effect(
	EngineProcessFactory,
	Effect.gen(function* () {
		const environment = yield* EngineProcessEnvironment;

		return {
			Spawn: (input) =>
				Effect.gen(function* () {
					yield* EnsureSpawnWorkingDirectory(input);
					const claim_token = yield* environment.MakeClaimToken;
					const AcquireOwned =
						environment.platform === "win32"
							? SpawnWindowsProcess(input, environment, claim_token)
							: Effect.try({
									try: () => spawn_posix_process(input),
									catch: (cause) =>
										new EngineProcessError({ cause, operation: "spawn" }),
								});
					const owned = yield* Effect.acquireRelease(AcquireOwned, (owned) =>
						owned.process_exit.IsClosed()
							? owned.process_exit.Cleanup
							: owned
									.Terminate("SIGKILL")
									.pipe(
										Effect.ignore,
										Effect.andThen(owned.process_exit.Cleanup),
									),
					);

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
							if (environment.platform === "win32") {
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
		};
	}),
);

export const EngineProcessFactoryLive = EngineProcessFactoryLayer.pipe(
	Layer.provide(EngineProcessEnvironmentLive),
);
