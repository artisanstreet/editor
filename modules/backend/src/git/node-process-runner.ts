import { Buffer } from "node:buffer";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { finished } from "node:stream";

import { Effect, Layer } from "effect";

import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "./process-runner";

/** Configures the maximum retained output for Node child processes. */
export interface NodeProcessRunnerOptions {
	readonly kill_timeout_ms?: number;
	readonly max_stderr_bytes?: number;
	readonly max_stdin_bytes?: number;
	readonly max_stdout_bytes?: number;
}

interface OutputCapture {
	readonly append: (chunk: Buffer) => void;
	readonly result: () => {
		readonly bytes: Uint8Array;
		readonly total_bytes: number;
		readonly truncated: boolean;
	};
}

function process_error(
	input: ProcessRunnerInput,
	operation: ProcessRunnerError["operation"],
	cause: unknown,
) {
	return new ProcessRunnerError({ cause, command: input.command, operation });
}

function is_valid_limit(value: number) {
	return Number.isSafeInteger(value) && value >= 0;
}

function make_capture(limit: number): OutputCapture {
	const chunks: Array<Buffer> = [];
	let retained_bytes = 0;
	let total_bytes = 0;

	return {
		append: (chunk) => {
			total_bytes += chunk.byteLength;

			if (retained_bytes >= limit) {
				return;
			}

			const remaining = limit - retained_bytes;
			const retained = chunk.byteLength <= remaining ? chunk : chunk.subarray(0, remaining);

			chunks.push(retained);
			retained_bytes += retained.byteLength;
		},
		result: () => ({
			bytes: Buffer.concat(chunks, retained_bytes),
			total_bytes,
			truncated: total_bytes > retained_bytes,
		}),
	};
}

function remove_process_listeners(
	child: ChildProcessWithoutNullStreams,
	on_close: (exit_code: number | null) => void,
	on_error: (cause: Error) => void,
	on_stderr: (chunk: Buffer) => void,
	on_stdout: (chunk: Buffer) => void,
) {
	child.removeListener("close", on_close);
	child.removeListener("error", on_error);
	child.stderr.removeListener("data", on_stderr);
	child.stdout.removeListener("data", on_stdout);
}

/** Builds the bounded Node process runner used by the Git adapter. */
export function make_node_process_runner_layer(options: NodeProcessRunnerOptions = {}) {
	const kill_timeout_ms = options.kill_timeout_ms ?? 1_000;
	const default_stderr_limit = options.max_stderr_bytes ?? 256 * 1024;
	const default_stdin_limit = options.max_stdin_bytes ?? 8 * 1024 * 1024;
	const default_stdout_limit = options.max_stdout_bytes ?? 8 * 1024 * 1024;

	return Layer.succeed(ProcessRunner, {
		Run: (input) =>
			Effect.gen(function* () {
				const stderr_limit = input.max_stderr_bytes ?? default_stderr_limit;
				const stdout_limit = input.max_stdout_bytes ?? default_stdout_limit;
				const stdin_bytes = input.stdin?.byteLength ?? 0;

				if (
					!Number.isSafeInteger(kill_timeout_ms) ||
					kill_timeout_ms <= 0 ||
					kill_timeout_ms > 60_000 ||
					!is_valid_limit(stderr_limit) ||
					!is_valid_limit(default_stdin_limit) ||
					stdin_bytes > default_stdin_limit ||
					!is_valid_limit(stdout_limit)
				) {
					return yield* Effect.fail(
						process_error(
							input,
							"configuration",
							new Error(
								"input and output limits must be non-negative safe integers, stdin must not exceed max_stdin_bytes, and kill_timeout_ms must be between 1 and 60000",
							),
						),
					);
				}

				const stdin = input.stdin === undefined ? undefined : Buffer.from(input.stdin);

				const child = yield* Effect.try({
					try: () =>
						spawn(input.command, [...input.args], {
							cwd: input.cwd,
							env:
								input.environment === undefined
									? process.env
									: { ...process.env, ...input.environment },
							shell: false,
							windowsHide: true,
						}),
					catch: (cause) => process_error(input, "spawn", cause),
				});

				return yield* Effect.callback<ProcessRunnerResult, ProcessRunnerError>((resume) => {
					const stderr = make_capture(stderr_limit);
					const stdout = make_capture(stdout_limit);
					const requires_stdin_delivery = stdin !== undefined;
					let child_closed = false;
					let child_exit_code = -1;
					let completed = false;
					let process_failure: ProcessRunnerError | undefined;
					let stdin_escape_timeout: ReturnType<typeof setTimeout> | undefined;
					let stdin_failure: ProcessRunnerError | undefined;
					let stdin_force_kill_timeout: ReturnType<typeof setTimeout> | undefined;
					let stdin_settled = false;
					let cleanup_stdin_listeners = () => {};
					/** Retained when a failed child cannot finish closing its inherited pipes. */
					const ignore_abandoned_error = () => {};

					const complete = (
						result: Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
					) => {
						if (completed) {
							return;
						}

						completed = true;
						remove_process_listeners(child, on_close, on_error, on_stderr, on_stdout);
						child.stdin.removeListener("error", on_stdin_error);
						cleanup_stdin_listeners();

						if (stdin_force_kill_timeout !== undefined) {
							clearTimeout(stdin_force_kill_timeout);
						}
						if (stdin_escape_timeout !== undefined) {
							clearTimeout(stdin_escape_timeout);
						}

						resume(result);
					};
					const abandon_after_stdin_failure = () => {
						if (completed || stdin_failure === undefined) {
							return;
						}

						child.on("error", ignore_abandoned_error);
						child.stdin.on("error", ignore_abandoned_error);
						child.stderr.destroy();
						child.stdin.destroy();
						child.stdout.destroy();
						complete(Effect.fail(stdin_failure));
					};
					const terminate_after_stdin_failure = () => {
						if (child.exitCode !== null || child.signalCode !== null) {
							stdin_escape_timeout ??= setTimeout(
								abandon_after_stdin_failure,
								kill_timeout_ms * 2,
							);

							return;
						}

						child.kill();
						stdin_force_kill_timeout ??= setTimeout(() => {
							if (child.exitCode === null && child.signalCode === null) {
								child.kill("SIGKILL");
							}
						}, kill_timeout_ms);
						stdin_escape_timeout ??= setTimeout(
							abandon_after_stdin_failure,
							kill_timeout_ms * 2,
						);
					};
					const record_stdin_failure = (cause: Error) => {
						if (!requires_stdin_delivery || stdin_failure !== undefined) {
							return;
						}

						stdin_failure = process_error(input, "stdin", cause);
						terminate_after_stdin_failure();
					};
					const complete_if_settled = () => {
						if (completed || !child_closed || !stdin_settled) {
							return;
						}

						if (process_failure !== undefined) {
							complete(Effect.fail(process_failure));

							return;
						}

						if (stdin_failure !== undefined) {
							complete(Effect.fail(stdin_failure));

							return;
						}

						const captured_stderr = stderr.result();
						const captured_stdout = stdout.result();

						complete(
							Effect.succeed({
								exit_code: child_exit_code,
								stderr: captured_stderr.bytes,
								stderr_bytes: captured_stderr.total_bytes,
								stderr_truncated: captured_stderr.truncated,
								stdout: captured_stdout.bytes,
								stdout_bytes: captured_stdout.total_bytes,
								stdout_truncated: captured_stdout.truncated,
							}),
						);
					};
					const on_stderr = (chunk: Buffer) => stderr.append(chunk);
					const on_stdin_error = (cause: Error) => record_stdin_failure(cause);
					const on_stdout = (chunk: Buffer) => stdout.append(chunk);
					const on_error = (cause: Error) => {
						process_failure ??= process_error(input, "spawn", cause);
					};
					const on_close = (exit_code: number | null) => {
						child_closed = true;
						child_exit_code = exit_code ?? -1;

						complete_if_settled();
					};

					child.stderr.on("data", on_stderr);
					child.stdin.on("error", on_stdin_error);
					child.stdout.on("data", on_stdout);
					child.once("error", on_error);
					child.once("close", on_close);
					cleanup_stdin_listeners = finished(child.stdin, (cause) => {
						stdin_settled = true;

						if (cause !== undefined && cause !== null) {
							record_stdin_failure(cause);
						}

						complete_if_settled();
					});
					child.stdin.end(stdin);

					return Effect.callback<void>((cleanup_resume) => {
						if (completed) {
							cleanup_resume(Effect.void);

							return;
						}

						completed = true;
						remove_process_listeners(child, on_close, on_error, on_stderr, on_stdout);

						let cleanup_completed = false;
						let escape_timeout: ReturnType<typeof setTimeout> | undefined;
						let force_kill_timeout: ReturnType<typeof setTimeout> | undefined;
						/** Retained after escape so abandoned child and stdin errors remain handled. */
						const ignore_cleanup_error = () => {};
						const complete_cleanup = (child_is_closed: boolean) => {
							if (cleanup_completed) {
								return;
							}

							cleanup_completed = true;
							child.removeListener("close", on_cleanup_close);

							if (child_is_closed) {
								child.removeListener("error", ignore_cleanup_error);
							} else {
								child.stderr.destroy();
								child.stdin.destroy();
								child.stdout.destroy();
							}

							if (force_kill_timeout !== undefined) {
								clearTimeout(force_kill_timeout);
							}

							if (escape_timeout !== undefined) {
								clearTimeout(escape_timeout);
							}

							cleanup_resume(Effect.void);
						};
						const on_cleanup_close = () => complete_cleanup(true);

						child.on("error", ignore_cleanup_error);
						child.stdin.on("error", ignore_cleanup_error);
						child.stdin.removeListener("error", on_stdin_error);
						cleanup_stdin_listeners();
						child.stderr.resume();
						child.stdin.destroy();
						child.stdout.resume();

						if (stdin_force_kill_timeout !== undefined) {
							clearTimeout(stdin_force_kill_timeout);
						}

						if (child.exitCode === null && child.signalCode === null) {
							force_kill_timeout = setTimeout(() => {
								if (child.exitCode === null && child.signalCode === null) {
									child.kill("SIGKILL");
								}
							}, kill_timeout_ms);
						}

						escape_timeout = setTimeout(
							() => complete_cleanup(false),
							kill_timeout_ms * 2,
						);

						if (child_closed) {
							complete_cleanup(true);
						} else {
							child.once("close", on_cleanup_close);
						}

						if (!child_closed && child.exitCode === null && child.signalCode === null) {
							child.kill();
						}
					});
				});
			}),
	});
}

/** Provides bounded production process execution with conservative defaults. */
export const NodeProcessRunnerLive = make_node_process_runner_layer();
