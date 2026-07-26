import { Buffer } from "node:buffer";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";

import { Effect, Layer } from "effect";

import {
	ProcessRunner,
	ProcessRunnerError,
	type ProcessRunnerInput,
	type ProcessRunnerResult,
} from "./process-runner";
import { ProcessEnvironment, ProcessEnvironmentLive } from "../runtime/process-environment";

/** Configures the maximum retained output for Node child processes. */
export interface NodeProcessRunnerOptions {
	readonly kill_timeout_ms?: number;
	readonly max_stderr_bytes?: number;
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

function remove_listeners(
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
	const default_stdout_limit = options.max_stdout_bytes ?? 8 * 1024 * 1024;

	return Layer.effect(
		ProcessRunner,
		Effect.gen(function* () {
			const process_environment = yield* ProcessEnvironment;
			return {
				Run: (input) =>
					Effect.gen(function* () {
						const inherited_environment = yield* process_environment.Variables;
						const stderr_limit = input.max_stderr_bytes ?? default_stderr_limit;
						const stdout_limit = input.max_stdout_bytes ?? default_stdout_limit;

						if (
							!Number.isSafeInteger(kill_timeout_ms) ||
							kill_timeout_ms <= 0 ||
							kill_timeout_ms > 60_000 ||
							!is_valid_limit(stderr_limit) ||
							!is_valid_limit(stdout_limit)
						) {
							return yield* Effect.fail(
								process_error(
									input,
									"configuration",
									new Error(
										"output limits must be non-negative safe integers and kill_timeout_ms must be between 1 and 60000",
									),
								),
							);
						}

						const child = yield* Effect.try({
							try: () =>
								spawn(input.command, [...input.args], {
									cwd: input.cwd,
									env: { ...inherited_environment, ...input.environment },
									shell: false,
									windowsHide: true,
								}),
							catch: (cause) => process_error(input, "spawn", cause),
						});

						return yield* Effect.callback<ProcessRunnerResult, ProcessRunnerError>(
							(resume) => {
								const stderr = make_capture(stderr_limit);
								const stdout = make_capture(stdout_limit);
								let completed = false;

								const complete = (
									result: Effect.Effect<ProcessRunnerResult, ProcessRunnerError>,
								) => {
									if (completed) {
										return;
									}

									completed = true;
									remove_listeners(
										child,
										on_close,
										on_error,
										on_stderr,
										on_stdout,
									);
									resume(result);
								};
								const on_stderr = (chunk: Buffer) => stderr.append(chunk);
								const on_stdout = (chunk: Buffer) => stdout.append(chunk);
								const on_error = (cause: Error) =>
									complete(Effect.fail(process_error(input, "spawn", cause)));
								const on_close = (exit_code: number | null) => {
									const captured_stderr = stderr.result();
									const captured_stdout = stdout.result();

									complete(
										Effect.succeed({
											exit_code: exit_code ?? -1,
											stderr: captured_stderr.bytes,
											stderr_bytes: captured_stderr.total_bytes,
											stderr_truncated: captured_stderr.truncated,
											stdout: captured_stdout.bytes,
											stdout_bytes: captured_stdout.total_bytes,
											stdout_truncated: captured_stdout.truncated,
										}),
									);
								};

								child.stderr.on("data", on_stderr);
								child.stdout.on("data", on_stdout);
								child.once("error", on_error);
								child.once("close", on_close);

								return Effect.gen(function* () {
									if (completed) {
										return;
									}

									completed = true;
									remove_listeners(
										child,
										on_close,
										on_error,
										on_stderr,
										on_stdout,
									);

									if (child.exitCode !== null || child.signalCode !== null) {
										return;
									}

									child.stderr.resume();
									child.stdout.resume();

									const WaitAfterKill = (signal?: NodeJS.Signals) =>
										Effect.callback<void>((cleanup_resume) => {
											const complete_cleanup = () =>
												cleanup_resume(Effect.void);
											const ignore_cleanup_error = () => {};

											child.once("close", complete_cleanup);
											child.once("error", ignore_cleanup_error);
											child.kill(signal);

											return Effect.sync(() => {
												child.removeListener("close", complete_cleanup);
												child.removeListener("error", ignore_cleanup_error);
											});
										});

									const exited_gracefully = yield* WaitAfterKill().pipe(
										Effect.as(true),
										Effect.timeoutOrElse({
											duration: kill_timeout_ms,
											orElse: () => Effect.succeed(false),
										}),
									);

									if (
										exited_gracefully ||
										child.exitCode !== null ||
										child.signalCode !== null
									) {
										return;
									}

									yield* WaitAfterKill("SIGKILL").pipe(
										Effect.timeoutOrElse({
											duration: kill_timeout_ms,
											orElse: () => Effect.void,
										}),
									);
								});
							},
						);
					}),
			};
		}),
	).pipe(Layer.provide(ProcessEnvironmentLive));
}

/** Provides bounded production process execution with conservative defaults. */
export const NodeProcessRunnerLive = make_node_process_runner_layer();
