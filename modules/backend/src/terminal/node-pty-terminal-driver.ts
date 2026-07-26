import { Buffer } from "node:buffer";

import { Cause, Deferred, Effect, FiberSet, Layer, Queue, Ref, Scope, Stream } from "effect";
import { spawn, type IDisposable, type IPty } from "node-pty";

import {
	TerminalDriver,
	TerminalDriverError,
	type TerminalDriverExit,
	type TerminalDriverHandle,
	type TerminalDriverOpenInput,
	type TerminalDriverOperation,
} from "./terminal-driver";
import { ProcessEnvironment, ProcessEnvironmentLive } from "../runtime/process-environment";

/** Configures the bounded native PTY adapter. */
export interface NodePtyTerminalDriverOptions {
	readonly close_timeout_ms?: number;
	readonly output_capacity?: number;
	readonly output_chunk_bytes?: number;
}

interface TerminalState {
	readonly exit?: TerminalDriverExit;
	readonly requested_reason?: "closed" | "killed";
}

function terminal_error(operation: TerminalDriverOperation, cause: unknown) {
	return new TerminalDriverError({ cause, operation });
}

function validate_positive(option: string, value: number) {
	return Number.isSafeInteger(value) && value > 0
		? Effect.void
		: Effect.fail(
				terminal_error("configuration", new Error(`${option} must be a positive integer`)),
			);
}

function validate_open_input(input: TerminalDriverOpenInput, output_capacity: number) {
	return Effect.gen(function* () {
		yield* validate_positive("cols", input.cols);
		yield* validate_positive("rows", input.rows);
		yield* validate_positive("output_capacity", output_capacity);

		if (input.executable.length === 0) {
			return yield* Effect.fail(
				terminal_error("configuration", new Error("executable must not be empty")),
			);
		}

		if (input.cwd.length === 0) {
			return yield* Effect.fail(
				terminal_error("configuration", new Error("cwd must not be empty")),
			);
		}
	});
}

function try_native(operation: TerminalDriverOperation, evaluate: () => void) {
	return Effect.try({
		try: evaluate,
		catch: (cause) => terminal_error(operation, cause),
	});
}

function make_handle(
	pty: IPty,
	close_timeout_ms: number,
	output_capacity: number,
	output_chunk_bytes: number,
): Effect.Effect<TerminalDriverHandle, never, Scope.Scope> {
	return Effect.gen(function* () {
		const output = yield* Queue.dropping<Uint8Array, Cause.Done<void>>(output_capacity);
		const exit = yield* Deferred.make<TerminalDriverExit>();
		const state = yield* Ref.make<TerminalState>({});
		const run_callback = yield* FiberSet.makeRuntime();
		let data_listener: IDisposable | undefined;
		let exit_listener: IDisposable | undefined;

		const Finish = (terminal_exit: TerminalDriverExit) =>
			Effect.gen(function* () {
				const should_finish = yield* Ref.modify(state, (current) =>
					current.exit
						? ([false, current] as const)
						: ([true, { ...current, exit: terminal_exit }] as const),
				);

				if (!should_finish) {
					return;
				}

				data_listener?.dispose();
				exit_listener?.dispose();
				yield* Queue.end(output);
				yield* Deferred.succeed(exit, terminal_exit);
			});
		const EnsureOpen = (operation: TerminalDriverOperation) =>
			Ref.get(state).pipe(
				Effect.flatMap((current) =>
					current.exit
						? Effect.fail(
								terminal_error(operation, new Error("terminal is already closed")),
							)
						: Effect.void,
				),
			);
		const KillNative = (reason: "closed" | "killed", signal?: string) =>
			Effect.gen(function* () {
				yield* EnsureOpen(reason === "closed" ? "close" : "kill");
				yield* Ref.update(state, (current) => ({ ...current, requested_reason: reason }));
				yield* try_native(reason === "closed" ? "close" : "kill", () => pty.kill(signal));
			});

		data_listener = pty.onData((data) => {
			const bytes = new TextEncoder().encode(data);

			for (let offset = 0; offset < bytes.length; offset += output_chunk_bytes) {
				const chunk = bytes.slice(offset, offset + output_chunk_bytes);

				if (Queue.offerUnsafe(output, chunk)) {
					continue;
				}

				const terminal_exit: TerminalDriverExit = {
					exit_code: null,
					reason: "output_overflow",
					signal: null,
				};

				run_callback(
					try_native("output", () => pty.kill()).pipe(
						Effect.ignore,
						Effect.andThen(Finish(terminal_exit)),
					),
				);

				return;
			}
		});
		exit_listener = pty.onExit(({ exitCode, signal }) => {
			run_callback(
				Ref.get(state).pipe(
					Effect.flatMap((current) =>
						Finish({
							exit_code: exitCode,
							reason: current.requested_reason ?? "exited",
							signal: signal ?? null,
						}),
					),
				),
			);
		});

		const Clear = EnsureOpen("clear").pipe(
			Effect.andThen(try_native("clear", () => pty.clear())),
		);
		const Close = Ref.get(state).pipe(
			Effect.flatMap((current) => {
				if (current.exit) {
					return Effect.void;
				}

				const fallback: TerminalDriverExit = {
					exit_code: null,
					reason: "closed",
					signal: null,
				};

				return KillNative("closed").pipe(
					Effect.andThen(
						Deferred.await(exit).pipe(
							Effect.timeoutOrElse({
								duration: close_timeout_ms,
								orElse: () => Finish(fallback).pipe(Effect.as(fallback)),
							}),
						),
					),
					Effect.asVoid,
				);
			}),
		);
		const Kill = (signal?: string) => KillNative("killed", signal);
		const Resize = (cols: number, rows: number) =>
			Effect.gen(function* () {
				yield* validate_positive("cols", cols);
				yield* validate_positive("rows", rows);
				yield* EnsureOpen("resize");
				yield* try_native("resize", () => pty.resize(cols, rows));
			});
		const Write = (chunk: Uint8Array) =>
			EnsureOpen("write").pipe(
				Effect.andThen(try_native("write", () => pty.write(Buffer.from(chunk)))),
			);

		yield* Effect.addFinalizer(() => Close.pipe(Effect.ignore));

		return {
			Clear,
			Close,
			Exit: Deferred.await(exit),
			Kill,
			Output: Stream.fromQueue(output),
			Resize,
			Write,
			pid: pty.pid,
		};
	});
}

/** Builds the native node-pty implementation used by the desktop backend process. */
export function make_node_pty_terminal_driver_layer(options: NodePtyTerminalDriverOptions = {}) {
	const close_timeout_ms = options.close_timeout_ms ?? 1_000;
	const output_capacity = options.output_capacity ?? 512;
	const output_chunk_bytes = options.output_chunk_bytes ?? 16_384;

	return Layer.effect(
		TerminalDriver,
		Effect.gen(function* () {
			const process_environment = yield* ProcessEnvironment;
			return {
				Open: (input) =>
					Effect.gen(function* () {
						const inherited_environment = yield* process_environment.Variables;
						yield* validate_positive("close_timeout_ms", close_timeout_ms);
						yield* validate_open_input(input, output_capacity);
						yield* validate_positive("output_chunk_bytes", output_chunk_bytes);

						const pty = yield* Effect.try({
							try: () =>
								spawn(input.executable, [...input.args], {
									cols: input.cols,
									cwd: input.cwd,
									env: { ...inherited_environment, ...input.env },
									handleFlowControl: false,
									name: "xterm-256color",
									rows: input.rows,
									...(process.platform === "win32"
										? { useConpty: true, useConptyDll: true }
										: {}),
								}),
							catch: (cause) => terminal_error("spawn", cause),
						});

						return yield* make_handle(
							pty,
							close_timeout_ms,
							output_capacity,
							output_chunk_bytes,
						);
					}),
			};
		}),
	).pipe(Layer.provide(ProcessEnvironmentLive));
}

/** Provides the production node-pty driver with bounded output buffering. */
export const NodePtyTerminalDriverLive = make_node_pty_terminal_driver_layer();
