import { Buffer } from "node:buffer";

import { Deferred, Effect, Exit, FileSystem, Ref, Scope, Semaphore, Stream } from "effect";

import {
	type EngineCapabilities,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineFailure,
	type EngineObservation,
	type EngineOpenInput,
	type EngineRun,
	EngineCommandIdConflictError,
	EngineConfigurationError,
	EngineProcessError,
	EngineProtocolError,
	EngineRunClosedError,
	EngineUnsupportedCommandError,
} from "../../engine";
import {
	ClassifyCodexExecSemanticOutcome,
	NormaliseCodexExecEvent,
	type CodexExecSemanticOutcome,
} from "../exec-normalizer";
import {
	CodexJsonlFramer,
	CodexJsonlMalformedLineError,
	CodexJsonlOversizedLineError,
	type CodexJsonlDecode,
} from "../jsonl";
import { WatchEngineInactivity } from "../../process/inactivity-deadline";
import type { CodexProcessFactory } from "../process";
import { MakeCodexExecSpawn } from "./exec-argv";
import { codex_exec_protocol_version, codex_exec_transport } from "./exec-contract";
import { MakeCodexExecEventBuffer } from "./exec-event-buffer";

/** Configures one bounded `codex exec --json` process lifecycle. */
export interface CodexExecRunOptions {
	readonly capabilities: EngineCapabilities;
	readonly event_capacity: number;
	readonly executable_args: ReadonlyArray<string>;
	readonly executable: string;
	readonly fallback_reason: string;
	readonly file_system: FileSystem.FileSystem;
	readonly factory: typeof CodexProcessFactory.Service;
	readonly max_frame_bytes: number;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	/** Silence that settles a run as stalled; every observation re-arms it. */
	readonly inactivity_ms: number;
}

interface ExecProcessState {
	readonly command_intents: ReadonlyMap<string, string>;
	readonly next_frame_sequence: number;
	readonly semantic_outcome: CodexExecSemanticOutcome;
	readonly stderr_bytes: number;
	readonly stdout_bytes: number;
}

function command_intent(command: EngineCommand) {
	return JSON.stringify([command._tag]);
}

function ValidatePositiveOption(option: string, value: number) {
	return Number.isSafeInteger(value) && value > 0
		? Effect.void
		: Effect.fail(new EngineConfigurationError({ engine_id: "codex", option, value }));
}

function make_diagnostic(
	artisan_run_id: string,
	message: string,
	level: "info" | "warning" | "error",
	frame: unknown,
	raw_frame_base64?: string,
): EngineObservation {
	return {
		_tag: "process_diagnostic",
		artisan_run_id,
		level,
		message,
		observation_id: `${artisan_run_id}:exec:diagnostic`,
		raw: {
			engine_id: "codex",
			frame,
			protocol_version: codex_exec_protocol_version,
			...(raw_frame_base64 === undefined ? {} : { raw_frame_base64 }),
			transport: codex_exec_transport,
		},
		sequence: 0,
	};
}

/** Opens one exec process and hides framing, timeout, cancellation, and terminal races. */
export function OpenCodexExecRun(
	options: CodexExecRunOptions,
	input: EngineOpenInput,
): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> {
	return Effect.gen(function* () {
		yield* ValidatePositiveOption("event_capacity", options.event_capacity);
		yield* ValidatePositiveOption("exec_max_frame_bytes", options.max_frame_bytes);
		yield* ValidatePositiveOption("exec_max_stderr_bytes", options.max_stderr_bytes);
		yield* ValidatePositiveOption("exec_max_stdout_bytes", options.max_stdout_bytes);
		yield* ValidatePositiveOption("exec_inactivity_ms", options.inactivity_ms);

		const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
			Scope.close(scope, Exit.succeed(undefined)),
		);
		const initial_content = input._tag === "start" ? input.initial_content : undefined;
		const image_paths =
			initial_content === undefined
				? []
				: yield* Effect.gen(function* () {
						const images = initial_content.filter((part) => part.type === "image");
						if (images.length === 0) return [];
						const directory = yield* options.file_system
							.makeTempDirectoryScoped({ prefix: "artisan-codex-images-" })
							.pipe(
								Scope.provide(run_scope),
								Effect.mapError(
									(cause) =>
										new EngineProcessError({ cause, operation: "write" }),
								),
							);
						return yield* Effect.forEach(images, (image, index) => {
							const extension =
								image.media_type === "image/jpeg"
									? "jpg"
									: image.media_type.slice(6);
							const path = `${directory}/image-${index}.${extension}`;
							return options.file_system.writeFile(path, image.bytes).pipe(
								Effect.as(path),
								Effect.mapError(
									(cause) =>
										new EngineProcessError({ cause, operation: "write" }),
								),
							);
						});
					});
		const spawn = yield* MakeCodexExecSpawn(
			input,
			options.executable,
			options.executable_args,
			image_paths,
		);
		const handle = yield* options.factory.Spawn(spawn);

		yield* Scope.addFinalizer(run_scope, handle.Close);

		const stdout_drained = yield* Deferred.make<void>();
		const stderr_drained = yield* Deferred.make<void>();
		const command_lock = yield* Semaphore.make(1);
		const process_state = yield* Ref.make<ExecProcessState>({
			command_intents: new Map(),
			next_frame_sequence: 0,
			semantic_outcome: "continue",
			stderr_bytes: 0,
			stdout_bytes: 0,
		});
		const event_buffer = yield* MakeCodexExecEventBuffer({
			artisan_run_id: input.artisan_run_id,
			capacity: options.event_capacity,
			CloseProcess: handle.Close,
		});
		const NextFrameSequence = Ref.modify(
			process_state,
			(current) =>
				[
					current.next_frame_sequence + 1,
					{ ...current, next_frame_sequence: current.next_frame_sequence + 1 },
				] as const,
		);
		const ProcessDecode = (decode: CodexJsonlDecode) =>
			Effect.gen(function* () {
				const frame_sequence = yield* NextFrameSequence;

				if (decode instanceof CodexJsonlOversizedLineError) {
					yield* event_buffer.Emit({
						_tag: "protocol_diagnostic",
						artisan_run_id: input.artisan_run_id,
						level: "error",
						message: `Codex exec JSONL frame exceeded ${decode.max_frame_bytes} bytes`,
						observation_id: `${input.artisan_run_id}:exec:${frame_sequence}:oversized`,
						raw: {
							engine_id: "codex",
							frame: {
								max_frame_bytes: decode.max_frame_bytes,
								prefix_base64: decode.prefix_base64,
								size_bytes: decode.size_bytes,
							},
							frame_sequence,
							protocol_version: codex_exec_protocol_version,
							transport: codex_exec_transport,
						},
						sequence: 0,
					});

					return yield* Effect.fail(
						new EngineProtocolError({
							engine_id: "codex",
							message: `Codex exec JSONL frame exceeded ${decode.max_frame_bytes} bytes`,
						}),
					);
				}

				if (decode instanceof CodexJsonlMalformedLineError) {
					yield* event_buffer.Emit({
						_tag: "protocol_diagnostic",
						artisan_run_id: input.artisan_run_id,
						level: "warning",
						message: `Malformed Codex exec JSONL: ${decode.message}`,
						observation_id: `${input.artisan_run_id}:exec:${frame_sequence}:malformed`,
						raw: {
							engine_id: "codex",
							frame: { line_base64: decode.line_base64 },
							frame_sequence,
							protocol_version: codex_exec_protocol_version,
							raw_frame_base64: decode.raw_frame_base64,
							transport: codex_exec_transport,
						},
						sequence: 0,
					});

					return;
				}

				const semantic_outcome = yield* ClassifyCodexExecSemanticOutcome(decode.payload);

				if (semantic_outcome === "failed") {
					yield* Ref.update(
						process_state,
						(current) =>
							({
								...current,
								semantic_outcome,
							}) satisfies ExecProcessState,
					);
				}

				const observations = yield* NormaliseCodexExecEvent({
					artisan_run_id: input.artisan_run_id,
					frame_sequence,
					payload: decode.payload,
					raw_frame_base64: decode.raw_frame_base64,
					turn_id: `exec:${input.artisan_run_id}:turn`,
				});

				yield* Effect.forEach(observations, event_buffer.Emit);
			});
		const framer = new CodexJsonlFramer({
			max_frame_bytes: options.max_frame_bytes,
		});
		const PumpStdout = Stream.fromAsyncIterable(
			handle.Stdout,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				Effect.gen(function* () {
					const total = yield* Ref.updateAndGet(process_state, (current) => ({
						...current,
						stdout_bytes: current.stdout_bytes + chunk.length,
					})).pipe(Effect.map((current) => current.stdout_bytes));

					if (total > options.max_stdout_bytes) {
						return yield* Effect.fail(
							new EngineProtocolError({
								engine_id: "codex",
								message: `Codex exec stdout exceeded ${options.max_stdout_bytes} bytes`,
							}),
						);
					}

					yield* Effect.forEach(framer.PushRecovering(chunk), ProcessDecode);
				}),
			),
			Effect.andThen(Effect.forEach(framer.FinishRecovering(), ProcessDecode)),
			Effect.catch((error) =>
				event_buffer
					.Emit(
						make_diagnostic(
							input.artisan_run_id,
							error instanceof Error ? error.message : "Codex exec stdout failed",
							"error",
							{ error },
						),
					)
					.pipe(Effect.andThen(event_buffer.Finish("failed"))),
			),
			Effect.ensuring(Deferred.succeed(stdout_drained, undefined).pipe(Effect.ignore)),
		);
		const stderr_decoder = new TextDecoder();
		const PumpStderr = Stream.fromAsyncIterable(
			handle.Stderr,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				Effect.gen(function* () {
					const total = yield* Ref.updateAndGet(process_state, (current) => ({
						...current,
						stderr_bytes: current.stderr_bytes + chunk.length,
					})).pipe(Effect.map((current) => current.stderr_bytes));

					if (total > options.max_stderr_bytes) {
						return yield* Effect.fail(
							new EngineProtocolError({
								engine_id: "codex",
								message: `Codex exec stderr exceeded ${options.max_stderr_bytes} bytes`,
							}),
						);
					}

					yield* event_buffer.Emit(
						make_diagnostic(
							input.artisan_run_id,
							stderr_decoder.decode(chunk, { stream: true }),
							"info",
							{ source: "stderr" },
							Buffer.from(chunk).toString("base64"),
						),
					);
				}),
			),
			Effect.catch((error) =>
				event_buffer
					.Emit(
						make_diagnostic(
							input.artisan_run_id,
							error instanceof Error ? error.message : "Codex exec stderr failed",
							"error",
							{ error },
						),
					)
					.pipe(Effect.andThen(event_buffer.Finish("failed"))),
			),
			Effect.ensuring(Deferred.succeed(stderr_drained, undefined).pipe(Effect.ignore)),
		);
		const WatchExit = Effect.gen(function* () {
			const process_exit = yield* handle.Exit;

			yield* Deferred.await(stdout_drained);
			yield* Deferred.await(stderr_drained);

			const semantic_outcome = (yield* Ref.get(process_state)).semantic_outcome;

			if (process_exit.code === 0) {
				yield* event_buffer.Finish(semantic_outcome === "failed" ? "failed" : "completed");

				return;
			}

			yield* event_buffer
				.Emit(
					make_diagnostic(
						input.artisan_run_id,
						`Codex exec exited with code ${String(process_exit.code)} and signal ${String(process_exit.signal)}`,
						"error",
						{ process_exit },
					),
				)
				.pipe(Effect.ignore);
			yield* event_buffer.Finish("failed");
		}).pipe(Effect.catch(() => event_buffer.Finish("failed")));
		const WatchInactivity = WatchEngineInactivity({
			Activity: event_buffer.Activity,
			Closed: event_buffer.Closed,
			/** `codex exec` is spawned per run, so it owes output for the whole run. */
			Expecting: Effect.succeed(true),
			inactivity_ms: options.inactivity_ms,
			OnStall: event_buffer
				.Emit(
					make_diagnostic(
						input.artisan_run_id,
						`Codex exec produced no output for ${options.inactivity_ms}ms and is treated as stalled`,
						"error",
						{ inactivity_ms: options.inactivity_ms },
					),
				)
				.pipe(Effect.andThen(event_buffer.Finish("failed"))),
		});

		yield* event_buffer.Emit(
			make_diagnostic(
				input.artisan_run_id,
				`Using codex exec --json fallback: ${options.fallback_reason}`,
				"warning",
				{
					capabilities: options.capabilities,
					reason: options.fallback_reason,
					source: "startup-transport-selection",
				},
			),
		);
		yield* Effect.forkScoped(PumpStdout).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(PumpStderr).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(WatchExit).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(WatchInactivity).pipe(Scope.provide(run_scope));
		yield* Scope.addFinalizer(run_scope, event_buffer.Finish("closed"));

		if (input._tag === "start") {
			const prompt =
				input.initial_content === undefined
					? input.initial_text
					: input.initial_content
							.map((part) =>
								part.type === "text" ? part.text : `[Attached image: ${part.name}]`,
							)
							.join("");
			yield* handle.Write(new TextEncoder().encode(prompt));
		}

		yield* handle.EndInput;

		const RememberCommand = (command_id: string, intent: string) =>
			Ref.update(process_state, (current) => ({
				...current,
				command_intents: new Map(current.command_intents).set(command_id, intent),
			}));
		const Send = (command: EngineCommand): Effect.Effect<void, EngineCommandFailure> =>
			Semaphore.withPermit(command_lock)(
				Effect.gen(function* () {
					const current = yield* Ref.get(process_state);
					const intent = command_intent(command);
					const accepted = current.command_intents.get(command.command_id);

					if (accepted !== undefined) {
						if (accepted === intent) {
							return;
						}

						return yield* Effect.fail(
							new EngineCommandIdConflictError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					}

					if ((yield* event_buffer.IsClosed) && command._tag !== "close") {
						return yield* Effect.fail(
							new EngineRunClosedError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					}

					if (command._tag === "close" || command._tag === "cancel") {
						yield* RememberCommand(command.command_id, intent);
						yield* event_buffer.Finish(
							command._tag === "cancel" ? "cancelled" : "closed",
						);

						return;
					}

					return yield* Effect.fail(
						new EngineUnsupportedCommandError({
							command: command._tag,
							command_id: command.command_id,
							engine_id: "codex",
						}),
					);
				}).pipe(Effect.uninterruptible),
			);
		const native_thread_id = `exec:${input.artisan_run_id}`;

		return {
			artisan_run_id: input.artisan_run_id,
			Closed: event_buffer.Closed,
			Events: event_buffer.Events,
			native_thread_id,
			resume_token: { native_thread_id },
			Send,
		} satisfies EngineRun;
	});
}
