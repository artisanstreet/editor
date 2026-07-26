import { NodeChildProcessSpawner, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Data, Deferred, Effect, Layer, Ref, Schema, Stream, type Duration } from "effect";
import { ChildProcess, ChildProcessSpawner } from "effect/unstable/process";
import { ProcessEnvironment, ProcessEnvironmentLive } from "../runtime/process-environment";

const maximum_retained_output_bytes = 32 * 1024 * 1024;
const maximum_stdin_bytes = 4 * 1024 * 1024;

const GitArgument = Schema.String.check(
	Schema.makeFilter<string>((argument) =>
		argument.includes("\0") ? "Expected a Git argument without NUL bytes" : undefined,
	),
);

const GitCommandInput = Schema.Struct({
	args: Schema.Array(GitArgument).check(Schema.isMaxLength(256)),
	cwd: Schema.NonEmptyString.check(
		Schema.makeFilter<string>((cwd) =>
			cwd.includes("\0") ? "Expected a Git working directory without NUL bytes" : undefined,
		),
	),
	max_stderr_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(maximum_retained_output_bytes),
	),
	max_stdin_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(maximum_stdin_bytes),
	),
	max_stdout_bytes: Schema.Int.check(
		Schema.isGreaterThanOrEqualTo(0),
		Schema.isLessThanOrEqualTo(maximum_retained_output_bytes),
	),
	mode: Schema.Literals(["mutation", "read"]),
	stdin: Schema.optional(Schema.Uint8Array),
});

/** Configures one bounded, shell-free Git invocation at a canonical root. */
export type GitCommandInput = typeof GitCommandInput.Type;

/** Contains retained bytes and the total count observed while draining one stream. */
export interface GitCommandOutput {
	readonly bytes: Uint8Array;
	readonly total_bytes: number;
	readonly truncated: boolean;
}

/** Contains one Git subprocess result without interpreting non-zero exit codes. */
export interface GitCommandResult {
	readonly exit_code: number;
	readonly output_limit_channel?: "stderr" | "stdout";
	readonly stderr: GitCommandOutput;
	readonly stdout: GitCommandOutput;
	readonly termination?: "output_limit" | "timeout";
}

export type GitCommandExecutorOperation = "configuration" | "process";

/** Conceals platform process errors behind a Git-specific infrastructure failure. */
export class GitCommandExecutorError extends Data.TaggedError("GitCommandExecutorError")<{
	readonly cause: unknown;
	readonly operation: GitCommandExecutorOperation;
}> {}

/** Runs only the configured Git executable; callers cannot select an arbitrary process. */
export class GitCommandExecutor extends Context.Service<
	GitCommandExecutor,
	{
		readonly Run: (
			input: GitCommandInput,
		) => Effect.Effect<GitCommandResult, GitCommandExecutorError>;
	}
>()("Artisan/GitCommandExecutor") {}

export interface GitCommandExecutorOptions {
	readonly command?: string;
	readonly force_kill_after?: Duration.Input;
	readonly timeout?: Duration.Input;
}

interface CaptureState {
	readonly chunks: ReadonlyArray<Uint8Array>;
	readonly retained_bytes: number;
	readonly total_bytes: number;
}

function append_capture(state: CaptureState, chunk: Uint8Array, limit: number): CaptureState {
	const total_bytes = Math.min(Number.MAX_SAFE_INTEGER, state.total_bytes + chunk.byteLength);
	const remaining = Math.max(0, limit - state.retained_bytes);

	if (remaining === 0) {
		return { ...state, total_bytes };
	}

	const retained = chunk.byteLength <= remaining ? chunk.slice() : chunk.slice(0, remaining);

	return {
		chunks: [...state.chunks, retained],
		retained_bytes: state.retained_bytes + retained.byteLength,
		total_bytes,
	};
}

function materialize_capture(state: CaptureState): GitCommandOutput {
	const bytes = new Uint8Array(state.retained_bytes);
	let offset = 0;

	for (const chunk of state.chunks) {
		bytes.set(chunk, offset);
		offset += chunk.byteLength;
	}

	return {
		bytes,
		total_bytes: state.total_bytes,
		truncated: state.total_bytes > state.retained_bytes,
	};
}

const InitialCaptureState = (): CaptureState => ({
	chunks: [] as ReadonlyArray<Uint8Array>,
	retained_bytes: 0,
	total_bytes: 0,
});

const Capture = <E, R>(
	stream: Stream.Stream<Uint8Array, E, R>,
	limit: number,
	state: Ref.Ref<CaptureState>,
	overflow: Deferred.Deferred<"stderr" | "stdout">,
	channel: "stderr" | "stdout",
) =>
	stream.pipe(
		Stream.runForEach((chunk) =>
			Ref.updateAndGet(state, (current) => append_capture(current, chunk, limit)).pipe(
				Effect.flatMap((next) =>
					next.total_bytes > next.retained_bytes
						? Deferred.succeed(overflow, channel).pipe(Effect.asVoid)
						: Effect.void,
				),
			),
		),
	);

function sanitized_environment(
	mode: GitCommandInput["mode"],
	environment: Readonly<Record<string, string | undefined>>,
) {
	const inherited = Object.fromEntries(
		Object.entries(environment).filter(
			([key, value]) => value !== undefined && !/^(?:GCM_|GIT_)/iu.test(key),
		),
	);

	return {
		...inherited,
		GCM_INTERACTIVE: "Never",
		GIT_FLUSH: "1",
		GIT_OPTIONAL_LOCKS: mode === "read" ? "0" : undefined,
		GIT_PAGER: "cat",
		GIT_TERMINAL_PROMPT: "0",
		LANG: "C",
		LC_ALL: "C",
		NO_COLOR: "1",
		PAGER: "cat",
	};
}

/** Builds the Git executor on Effect's scoped child-process service. */
export function make_git_command_executor_layer(options: GitCommandExecutorOptions = {}) {
	const command = options.command ?? "git";
	const force_kill_after = options.force_kill_after ?? "1 second";
	const timeout = options.timeout ?? "30 seconds";

	return Layer.effect(
		GitCommandExecutor,
		Effect.gen(function* () {
			const spawner = yield* ChildProcessSpawner.ChildProcessSpawner;
			const process_environment = yield* ProcessEnvironment;
			const Run = (input: GitCommandInput) =>
				Effect.gen(function* () {
					const inherited_environment = yield* process_environment.Variables;
					return yield* Schema.decodeUnknownEffect(GitCommandInput, {
						onExcessProperty: "error",
					})(input).pipe(
						Effect.mapError(
							(cause) =>
								new GitCommandExecutorError({ cause, operation: "configuration" }),
						),
						Effect.flatMap((request) => {
							if (
								request.stdin !== undefined &&
								request.stdin.byteLength > request.max_stdin_bytes
							) {
								return Effect.fail(
									new GitCommandExecutorError({
										cause: new Error(
											"Git stdin exceeded its configured byte limit",
										),
										operation: "configuration",
									}),
								);
							}

							const child = ChildProcess.make(command, request.args, {
								cwd: request.cwd,
								env: sanitized_environment(request.mode, inherited_environment),
								extendEnv: false,
								forceKillAfter: force_kill_after,
								shell: false,
								stderr: "pipe",
								stdin:
									request.stdin === undefined
										? "ignore"
										: Stream.make(request.stdin),
								stdout: "pipe",
							});

							return Effect.scoped(
								Effect.gen(function* () {
									const handle = yield* spawner.spawn(child);
									const overflow = yield* Deferred.make<"stderr" | "stdout">();
									const stdout_state = yield* Ref.make(InitialCaptureState());
									const stderr_state = yield* Ref.make(InitialCaptureState());
									const completed = Effect.all(
										[
											Capture(
												handle.stdout,
												request.max_stdout_bytes,
												stdout_state,
												overflow,
												"stdout",
											),
											Capture(
												handle.stderr,
												request.max_stderr_bytes,
												stderr_state,
												overflow,
												"stderr",
											),
											handle.exitCode,
										] as const,
										{ concurrency: "unbounded" },
									).pipe(
										Effect.map(([, , exit_code]) => ({
											exit_code,
											type: "exit" as const,
										})),
									);
									const output_limited = Deferred.await(overflow).pipe(
										Effect.map((channel) => ({
											channel,
											type: "output_limit" as const,
										})),
									);
									const timed_out = Effect.sleep(timeout).pipe(
										Effect.as({ type: "timeout" as const }),
									);
									const outcome = yield* Effect.raceFirst(
										Effect.raceFirst(completed, output_limited),
										timed_out,
									);
									const stdout = materialize_capture(
										yield* Ref.get(stdout_state),
									);
									const stderr = materialize_capture(
										yield* Ref.get(stderr_state),
									);

									return outcome.type === "exit"
										? { exit_code: outcome.exit_code, stderr, stdout }
										: {
												exit_code: -1,
												...(outcome.type === "output_limit"
													? { output_limit_channel: outcome.channel }
													: {}),
												stderr,
												stdout,
												termination: outcome.type,
											};
								}),
							).pipe(
								Effect.mapError(
									(cause) =>
										new GitCommandExecutorError({
											cause,
											operation: "process",
										}),
								),
							);
						}),
					);
				});

			return { Run };
		}),
	).pipe(Layer.provide(ProcessEnvironmentLive));
}

/** Builds the production Node Git executor from Effect filesystem, path, and process layers. */
export function make_node_git_command_executor_layer(options: GitCommandExecutorOptions = {}) {
	const node_process = NodeChildProcessSpawner.layer.pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
	);

	return make_git_command_executor_layer(options).pipe(Layer.provide(node_process));
}

export const NodeGitCommandExecutorLive = make_node_git_command_executor_layer();
