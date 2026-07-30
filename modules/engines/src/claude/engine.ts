import { Buffer } from "node:buffer";
import { randomUUID } from "node:crypto";

import {
	Context,
	Deferred,
	Effect,
	Encoding,
	Exit,
	Layer,
	Ref,
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	type Engine,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineDescriptor,
	type EngineFailure,
	type EngineObservation,
	type EngineOpenInput,
	type EngineProbe,
	type EngineRun,
	type EngineUserInputPart,
	EngineCommandIdConflictError,
	EngineConfigurationError,
	EngineProbeTimeoutError,
	EngineProcessError,
	EngineProtocolError,
	EngineRunClosedError,
	EngineUnavailableError,
	EngineUnsupportedCommandError,
	EngineUnsupportedOperationError,
	ValidateEngineGlobalGuidance,
} from "../engine";
import { EngineProcessFactory, type EngineProcessSpawnInput } from "../process/process";
import { MakeEngineEventBuffer } from "../process/event-buffer";
import {
	ClaudeJsonlFramer,
	ClaudeJsonlMalformedLineError,
	ClaudeJsonlOversizedLineError,
	type ClaudeJsonlDecode,
} from "./jsonl";
import { MakeClaudeUsage } from "./usage";
import {
	classify_claude_semantic_failure,
	is_claude_init_event,
	normalize_claude_event,
	read_claude_session_id,
	read_claude_stream_message_id,
} from "./normalizer";

/** Declares the deliberately limited, truthful Claude Code capability surface. @since 0.6.0 */
export const ClaudeEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "unsupported",
			reason: "The stream-json print mode does not expose an approval response channel.",
		},
		auth: {
			state: "supported",
			reason: "Uses the installed Claude Code subscription session.",
		},
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: { state: "supported" },
		global_guidance: {
			state: "unsupported",
			reason: "No provider-neutral global guidance mirror is wired for Claude Code.",
		},
		model_selection: {
			state: "supported",
			reason: "Native model identifiers pass through to the CLI's --model flag.",
		},
		native_continuation: {
			state: "supported",
			reason: "Same-engine model changes are explicitly target-model and Claude 2.1.220 gated.",
		},
		native_tools: {
			state: "experimental",
			reason: "Known public tool activity is normalized; provider-native details remain raw.",
		},
		probe: { state: "supported", reason: "Only --version and auth status are probed." },
		question: {
			state: "unsupported",
			reason: "Claude Code has no canonical question response mapping here.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported" },
		start: { state: "supported" },
		steer: {
			state: "unsupported",
			reason: "Stream-input messages are queued by Claude Code, not true steering.",
		},
		subagents: { state: "unsupported", reason: "Subagent events are not canonicalized." },
	},
	display_name: "Claude",
	id: "claude",
	transport: "claude-cli-stream-json",
};

/** Configures Claude CLI execution and all bounded process/protocol limits. @since 0.6.0 */
export interface ClaudeEngineOptions {
	readonly auth_timeout_ms?: number;
	/** Overrides Claude's config-dir resolution used by `Usage` (normally env `CLAUDE_CONFIG_DIR`, else `~/.claude`). */
	readonly claude_config_dir?: string;
	readonly event_capacity?: number;
	readonly executable?: string;
	readonly executable_args?: ReadonlyArray<string>;
	readonly max_frame_bytes?: number;
	readonly max_stderr_bytes?: number;
	readonly max_stdout_bytes?: number;
	readonly timeout_ms?: number;
	readonly version_timeout_ms?: number;
}

interface ClaudeConfiguredOptions {
	readonly auth_timeout_ms: number;
	readonly event_capacity: number;
	readonly executable: string;
	readonly executable_args: ReadonlyArray<string>;
	readonly max_frame_bytes: number;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly timeout_ms: number;
	readonly version_timeout_ms: number;
}

interface ClaudeSpawnInput extends EngineProcessSpawnInput {
	readonly session_id: string;
}

/** Provides the Claude Code engine service. @since 0.6.0 */
export class ClaudeEngine extends Context.Service<ClaudeEngine, Engine>()("Artisan/ClaudeEngine") {}

/** Names every native permission mode accepted by the Claude Code CLI. @since 0.6.0 */
export const claude_permission_modes = [
	"plan",
	"default",
	"acceptEdits",
	"auto",
	"bypassPermissions",
] as const;

/**
 * Probe phases default to 15 seconds because on Windows each spawn passes
 * through the intermediate Node process host and the npm `claude.cmd` shim;
 * live measurement puts one non-billable phase near ten seconds there.
 */
const defaults = {
	event_capacity: 256,
	executable: "claude",
	max_frame_bytes: 1_048_576,
	max_stderr_bytes: 1_048_576,
	max_stdout_bytes: 16 * 1_024 * 1_024,
	timeout_ms: 10 * 60 * 1_000,
	version_timeout_ms: 15_000,
	auth_timeout_ms: 15_000,
} as const;
const native_continuation_version = "2.1.220";

function fail_configuration(option: string, value: unknown) {
	return Effect.fail(new EngineConfigurationError({ engine_id: "claude", option, value }));
}

function validate_positive(options: ClaudeEngineOptions) {
	const values = {
		event_capacity: options.event_capacity ?? defaults.event_capacity,
		max_frame_bytes: options.max_frame_bytes ?? defaults.max_frame_bytes,
		max_stderr_bytes: options.max_stderr_bytes ?? defaults.max_stderr_bytes,
		max_stdout_bytes: options.max_stdout_bytes ?? defaults.max_stdout_bytes,
		timeout_ms: options.timeout_ms ?? defaults.timeout_ms,
		version_timeout_ms: options.version_timeout_ms ?? defaults.version_timeout_ms,
		auth_timeout_ms: options.auth_timeout_ms ?? defaults.auth_timeout_ms,
	};

	const invalid = Object.entries(values).find(
		([, value]) => !Number.isSafeInteger(value) || value <= 0,
	);

	return invalid === undefined
		? Effect.succeed(values)
		: fail_configuration(invalid[0], invalid[1]);
}

/**
 * Validates the caller's run context against what the Claude CLI natively
 * accepts. Canonical permission policies and global guidance are rejected
 * before any spawn because the adapter has no native mapping for them; only
 * the provider-owned permission mode and prompt-file options pass through.
 */
function validate_provider_options(input: EngineOpenInput) {
	const options = input.provider_options ?? {};
	const allowed = new Set([
		"claude.permission_mode",
		"claude.append_system_prompt_file",
		"claude.disable_tools",
		"claude.safe_mode",
	]);
	const unknown = Object.keys(options).find((key) => !allowed.has(key));

	if (unknown !== undefined) {
		return fail_configuration(`provider_options.${unknown}`, options[unknown]);
	}

	if (input.permission_policy !== undefined) {
		return fail_configuration("permission_policy", input.permission_policy);
	}

	const permission_mode = options["claude.permission_mode"];
	if (
		permission_mode !== undefined &&
		(typeof permission_mode !== "string" ||
			!(claude_permission_modes as ReadonlyArray<string>).includes(permission_mode))
	) {
		return fail_configuration("provider_options.claude.permission_mode", permission_mode);
	}

	const prompt_file = options["claude.append_system_prompt_file"];
	if (
		prompt_file !== undefined &&
		(typeof prompt_file !== "string" || prompt_file.length === 0)
	) {
		return fail_configuration("provider_options.claude.append_system_prompt_file", prompt_file);
	}
	const disable_tools = options["claude.disable_tools"];
	if (disable_tools !== undefined && disable_tools !== true) {
		return fail_configuration("provider_options.claude.disable_tools", disable_tools);
	}
	const safe_mode = options["claude.safe_mode"];
	if (safe_mode !== undefined && safe_mode !== true) {
		return fail_configuration("provider_options.claude.safe_mode", safe_mode);
	}

	return ValidateEngineGlobalGuidance("claude", input.global_guidance).pipe(
		Effect.andThen(
			input.global_guidance === undefined
				? Effect.succeed({
						disable_tools: disable_tools === true,
						permission_mode,
						prompt_file,
						safe_mode: safe_mode === true,
					})
				: Effect.fail(
						new EngineUnsupportedOperationError({
							engine_id: "claude",
							operation: "global_guidance",
						}),
					),
		),
	);
}

function make_spawn_input(
	input: EngineOpenInput,
	options: Required<Pick<ClaudeEngineOptions, "executable" | "executable_args">>,
	session_id: string,
) {
	return Effect.gen(function* () {
		const { disable_tools, permission_mode, prompt_file, safe_mode } =
			yield* validate_provider_options(input);
		const args = [
			...options.executable_args,
			...(input._tag === "resume" ? ["--resume", input.resume_token.native_thread_id] : []),
			"-p",
			"--output-format",
			"stream-json",
			"--input-format",
			"stream-json",
			"--verbose",
			"--include-partial-messages",
			...(safe_mode ? ["--safe-mode"] : []),
			...(disable_tools ? ["--tools", ""] : []),
			...(input._tag === "start" ? ["--session-id", session_id] : []),
			...(input.model === undefined ? [] : ["--model", input.model]),
			...(permission_mode === undefined ? [] : ["--permission-mode", permission_mode]),
			...(prompt_file === undefined ? [] : ["--append-system-prompt-file", prompt_file]),
		];

		return {
			args,
			command: options.executable,
			cwd: input.working_directory,
			env: process.env,
			session_id,
		} satisfies ClaudeSpawnInput;
	});
}

/**
 * Serializes the user turn as the Anthropic message content the CLI's
 * stream-json input accepts. Text-only turns stay a plain string exactly as
 * before; ordered text/image parts become native content blocks.
 */
function make_user_content(text: string, content: ReadonlyArray<EngineUserInputPart> | undefined) {
	return content === undefined
		? text
		: content.map((part) =>
				part.type === "text"
					? { text: part.text, type: "text" }
					: {
							source: {
								data: Encoding.encodeBase64(part.bytes),
								media_type: part.media_type,
								type: "base64",
							},
							type: "image",
						},
			);
}

function make_diagnostic(
	input: EngineOpenInput,
	message: string,
	frame: unknown,
	level: "info" | "warning" | "error" = "info",
	raw_frame_base64?: string,
): EngineObservation {
	return {
		_tag: "process_diagnostic",
		artisan_run_id: input.artisan_run_id,
		level,
		message,
		observation_id: `${input.artisan_run_id}:claude:diagnostic`,
		raw: {
			engine_id: "claude",
			frame,
			...(raw_frame_base64 === undefined ? {} : { raw_frame_base64 }),
			transport: "claude-cli-stream-json",
			protocol_version: "claude-stream-json-v1",
		},
		sequence: 0,
	};
}

function read_bounded(stream: AsyncIterable<Uint8Array>, max_bytes: number) {
	return Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.length;
				if (total > max_bytes)
					throw new EngineProtocolError({
						engine_id: "claude",
						message: `Claude probe output exceeded ${max_bytes} bytes`,
					});
				chunks.push(chunk);
			}
			return Buffer.concat(chunks);
		},
		catch: (cause) =>
			cause instanceof EngineProtocolError
				? cause
				: new EngineProcessError({ cause, operation: "read" }),
	});
}

function make_probe(
	factory: typeof EngineProcessFactory.Service,
	options: Required<
		Pick<
			ClaudeEngineOptions,
			| "executable"
			| "executable_args"
			| "version_timeout_ms"
			| "auth_timeout_ms"
			| "max_stdout_bytes"
			| "max_stderr_bytes"
		>
	>,
): Effect.Effect<EngineProbe, EngineFailure> {
	const run = (args: ReadonlyArray<string>, timeout_ms: number) =>
		Effect.scoped(
			Effect.gen(function* () {
				const handle = yield* factory.Spawn({
					command: options.executable,
					args: [...options.executable_args, ...args],
				});
				return yield* Effect.all(
					[
						read_bounded(handle.Stdout, options.max_stdout_bytes),
						read_bounded(handle.Stderr, options.max_stderr_bytes),
						handle.Exit,
					],
					{ concurrency: "unbounded" },
				).pipe(
					Effect.ensuring(handle.Close),
					Effect.timeoutOrElse({
						duration: timeout_ms,
						orElse: () =>
							Effect.fail(
								new EngineProbeTimeoutError({
									engine_id: "claude",
									phase: args[0] === "auth" ? "authentication" : "version",
									timeout_ms,
								}),
							),
					}),
				);
			}),
		);

	return Effect.gen(function* () {
		const [version_stdout, version_stderr, version_exit] = yield* run(
			["--version"],
			options.version_timeout_ms,
		);
		if (version_exit.code !== 0)
			return yield* Effect.fail(
				new EngineUnavailableError({
					engine_id: "claude",
					message: `Claude --version exited ${String(version_exit.code)}: ${new TextDecoder().decode(version_stderr)}`,
				}),
			);
		const version = new TextDecoder()
			.decode(version_stdout)
			.match(/\b\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\b/)?.[0];
		if (version === undefined)
			return yield* Effect.fail(
				new EngineUnavailableError({
					engine_id: "claude",
					message: "Claude --version did not contain a semantic version",
				}),
			);
		const [auth_stdout, auth_stderr, auth_exit] = yield* run(
			["auth", "status"],
			options.auth_timeout_ms,
		);
		const auth_status = Schema.decodeUnknownOption(
			Schema.fromJsonString(Schema.Struct({ loggedIn: Schema.Boolean })),
		)(new TextDecoder().decode(auth_stdout));
		const unknown = auth_status._tag === "None" || auth_exit.code !== 0;
		const authenticated = !unknown && auth_status._tag === "Some" && auth_status.value.loggedIn;
		const authentication = unknown
			? {
					reason:
						new TextDecoder().decode(auth_stderr).trim() ||
						"Claude auth status is unavailable",
					state: "unknown" as const,
				}
			: { state: authenticated ? ("authenticated" as const) : ("unauthenticated" as const) };
		return {
			authentication,
			capabilities: ClaudeEngineDescriptor.capabilities,
			descriptor: ClaudeEngineDescriptor,
			metadata: {},
			ready: authenticated,
			version,
		};
	});
}

function open_run(
	factory: typeof EngineProcessFactory.Service,
	options: ClaudeConfiguredOptions,
	input: EngineOpenInput,
): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> {
	return Effect.gen(function* () {
		const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
			Scope.close(scope, Exit.succeed(undefined)),
		);
		const native_session_id =
			input._tag === "resume" ? input.resume_token.native_thread_id : randomUUID();
		const spawn_input = yield* make_spawn_input(input, options, native_session_id);
		const handle = yield* factory.Spawn(spawn_input).pipe(Scope.provide(run_scope));
		yield* Scope.addFinalizer(run_scope, handle.Close);
		const prompt = input._tag === "start" ? input.initial_text : input.next_text;
		const content = input._tag === "start" ? input.initial_content : input.next_content;

		if (prompt !== undefined || (content !== undefined && content.length > 0)) {
			const message = JSON.stringify({
				type: "user",
				message: {
					role: "user",
					content: make_user_content(prompt ?? "", content),
				},
				parent_tool_use_id: null,
				session_id: spawn_input.session_id,
			});

			yield* handle.Write(new TextEncoder().encode(`${message}\n`));
		}

		yield* handle.EndInput;
		const event_buffer = yield* MakeEngineEventBuffer({
			artisan_run_id: input.artisan_run_id,
			capacity: options.event_capacity,
			CloseResource: handle.Close,
			make_terminal_observation: (state, sequence) => ({
				_tag: "run_terminal",
				artisan_run_id: input.artisan_run_id,
				observation_id: `${input.artisan_run_id}:claude:terminal`,
				raw: {
					engine_id: "claude",
					frame: { state },
					protocol_version: "claude-stream-json-v1",
					transport: "claude-cli-stream-json",
				},
				sequence,
				state,
			}),
		});
		const state = yield* Ref.make({
			frame_sequence: 0,
			semantic_failure: false,
			stdout_bytes: 0,
			stderr_bytes: 0,
			command_intents: new Map<string, string>(),
			session_id: spawn_input.session_id,
			init_seen: false,
			/** The message id announced by the newest `message_start` frame. */
			stream_message_id: undefined as string | undefined,
		});
		const stdout_drained = yield* Deferred.make<void>();
		const stderr_drained = yield* Deferred.make<void>();
		const framer = new ClaudeJsonlFramer({ max_frame_bytes: options.max_frame_bytes });

		const process_decode = (decode: ClaudeJsonlDecode) =>
			Effect.gen(function* () {
				const frame_sequence = yield* Ref.modify(
					state,
					(current) =>
						[
							current.frame_sequence + 1,
							{ ...current, frame_sequence: current.frame_sequence + 1 },
						] as const,
				);
				const session_id = read_claude_session_id(
					decode instanceof ClaudeJsonlMalformedLineError ||
						decode instanceof ClaudeJsonlOversizedLineError
						? undefined
						: decode.payload,
				);
				const current_state = yield* Ref.get(state);
				if (session_id !== undefined && session_id !== current_state.session_id) {
					yield* event_buffer.Emit(
						make_diagnostic(
							input,
							"Claude session identity mismatch",
							{ expected: current_state.session_id, received: session_id },
							"error",
						),
					);
					return yield* Effect.fail(
						new EngineProtocolError({
							engine_id: "claude",
							message: "Claude session identity mismatch",
						}),
					);
				}
				if (
					!(decode instanceof ClaudeJsonlMalformedLineError) &&
					!(decode instanceof ClaudeJsonlOversizedLineError) &&
					is_claude_init_event(decode.payload)
				)
					yield* Ref.update(state, (current) => ({ ...current, init_seen: true }));
				if (decode instanceof ClaudeJsonlOversizedLineError) {
					yield* event_buffer.Emit(
						make_diagnostic(
							input,
							decode.message,
							{
								size_bytes: decode.size_bytes,
								max_frame_bytes: decode.max_frame_bytes,
							},
							"error",
							decode.prefix_base64,
						),
					);
					return yield* Effect.fail(
						new EngineProtocolError({ engine_id: "claude", message: decode.message }),
					);
				}
				if (decode instanceof ClaudeJsonlMalformedLineError) {
					yield* event_buffer.Emit(
						make_diagnostic(
							input,
							decode.message,
							{ malformed: true },
							"warning",
							decode.raw_frame_base64,
						),
					);
					return;
				}
				if (classify_claude_semantic_failure(decode.payload))
					yield* Ref.update(state, (current) => ({ ...current, semantic_failure: true }));
				/**
				 * `message_start` is the only frame that names the message its
				 * deltas belong to, so the id is carried forward until the next
				 * message begins. Without it the completion would land on a
				 * second conversation item beside the streamed one.
				 */
				const announced_message_id = read_claude_stream_message_id(decode.payload);
				const stream_message_id = announced_message_id ?? current_state.stream_message_id;
				if (announced_message_id !== undefined)
					yield* Ref.update(state, (current) => ({
						...current,
						stream_message_id: announced_message_id,
					}));
				yield* Effect.forEach(
					normalize_claude_event({
						artisan_run_id: input.artisan_run_id,
						frame_sequence,
						payload: decode.payload,
						raw_frame_base64: decode.raw_frame_base64,
						...(stream_message_id === undefined ? {} : { stream_message_id }),
						turn_id: `claude:${input.artisan_run_id}:turn`,
					}),
					event_buffer.Emit,
				);
			});

		const pump_stdout = Stream.fromAsyncIterable(
			handle.Stdout,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				Effect.gen(function* () {
					const total = yield* Ref.updateAndGet(state, (current) => ({
						...current,
						stdout_bytes: current.stdout_bytes + chunk.length,
					})).pipe(Effect.map((current) => current.stdout_bytes));
					if (total > options.max_stdout_bytes)
						return yield* Effect.fail(
							new EngineProtocolError({
								engine_id: "claude",
								message: "Claude stdout limit exceeded",
							}),
						);
					yield* Effect.forEach(framer.PushRecovering(chunk), process_decode);
				}),
			),
			Effect.ensuring(
				Effect.forEach(framer.FinishRecovering(), process_decode)
					.pipe(Effect.ignore)
					.pipe(Effect.andThen(Deferred.succeed(stdout_drained, undefined))),
			),
			Effect.catch((error) =>
				event_buffer
					.Emit(make_diagnostic(input, String(error), { source: "stdout" }, "error"))
					.pipe(Effect.andThen(event_buffer.Finish("failed"))),
			),
		);
		const stderr_decoder = new TextDecoder();
		const pump_stderr = Stream.fromAsyncIterable(
			handle.Stderr,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((chunk) =>
				Effect.gen(function* () {
					const total = yield* Ref.updateAndGet(state, (current) => ({
						...current,
						stderr_bytes: current.stderr_bytes + chunk.length,
					})).pipe(Effect.map((current) => current.stderr_bytes));
					if (total > options.max_stderr_bytes)
						return yield* Effect.fail(
							new EngineProtocolError({
								engine_id: "claude",
								message: "Claude stderr limit exceeded",
							}),
						);
					yield* event_buffer.Emit(
						make_diagnostic(
							input,
							stderr_decoder.decode(chunk, { stream: true }),
							{ source: "stderr" },
							"info",
							Buffer.from(chunk).toString("base64"),
						),
					);
				}),
			),
			Effect.ensuring(
				event_buffer
					.Emit(
						make_diagnostic(
							input,
							stderr_decoder.decode(),
							{ source: "stderr.flush" },
							"info",
						),
					)
					.pipe(Effect.ignore)
					.pipe(Effect.andThen(Deferred.succeed(stderr_drained, undefined))),
			),
			Effect.catch((error) =>
				event_buffer
					.Emit(make_diagnostic(input, String(error), { source: "stderr" }, "error"))
					.pipe(Effect.andThen(event_buffer.Finish("failed"))),
			),
		);
		const watch_exit = handle.Exit.pipe(
			Effect.flatMap((exit) =>
				Effect.gen(function* () {
					yield* Deferred.await(stdout_drained);
					yield* Deferred.await(stderr_drained);
					const current = yield* Ref.get(state);
					if (exit.code !== 0 || exit.signal !== null)
						yield* event_buffer
							.Emit(
								make_diagnostic(
									input,
									`Claude exited with code ${String(exit.code)} and signal ${String(exit.signal)}`,
									{ exit },
									"error",
								),
							)
							.pipe(Effect.ignore);
					if (!current.init_seen)
						yield* event_buffer
							.Emit(
								make_diagnostic(
									input,
									"Claude stream did not contain system/init",
									{ exit },
									"error",
								),
							)
							.pipe(Effect.ignore);
					yield* event_buffer.Finish(
						exit.code === 0 && current.init_seen && !current.semantic_failure
							? "completed"
							: "failed",
					);
				}),
			),
			Effect.catch(() => event_buffer.Finish("failed")),
		);
		const watch_timeout = Effect.raceFirst(
			event_buffer.Closed.pipe(Effect.asVoid),
			Effect.sleep(options.timeout_ms).pipe(
				Effect.andThen(
					event_buffer.Emit(
						make_diagnostic(
							input,
							`Claude timed out after ${options.timeout_ms}ms`,
							{ timeout_ms: options.timeout_ms },
							"error",
						),
					),
				),
				Effect.andThen(event_buffer.Finish("failed")),
			),
		);
		yield* Effect.forkScoped(pump_stdout).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(pump_stderr).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(watch_exit).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(watch_timeout).pipe(Scope.provide(run_scope));
		yield* Scope.addFinalizer(run_scope, event_buffer.Finish("closed").pipe(Effect.ignore));

		const command_lock = yield* Semaphore.make(1);
		const Send = (command: EngineCommand): Effect.Effect<void, EngineCommandFailure> =>
			Semaphore.withPermit(command_lock)(
				Effect.gen(function* () {
					const intent = JSON.stringify(command);
					const current = yield* Ref.get(state);
					const prior = current.command_intents.get(command.command_id);
					if (prior !== undefined) {
						if (prior === intent) return;
						return yield* Effect.fail(
							new EngineCommandIdConflictError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					}
					if ((yield* event_buffer.IsClosed) && command._tag !== "close")
						return yield* Effect.fail(
							new EngineRunClosedError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					if (command._tag === "cancel" || command._tag === "close") {
						yield* Ref.update(state, (value) => ({
							...value,
							command_intents: new Map(value.command_intents).set(
								command.command_id,
								intent,
							),
						}));
						return yield* event_buffer.Finish(
							command._tag === "cancel" ? "cancelled" : "closed",
						);
					}
					return yield* Effect.fail(
						new EngineUnsupportedCommandError({
							engine_id: "claude",
							command: command._tag,
							command_id: command.command_id,
						}),
					);
				}),
			);
		return {
			artisan_run_id: input.artisan_run_id,
			Closed: event_buffer.Closed,
			Events: event_buffer.Events,
			native_thread_id: spawn_input.session_id,
			resume_token: { native_thread_id: spawn_input.session_id },
			Send,
		} satisfies EngineRun;
	});
}

/**
 * Builds a Claude Code engine using only the installed CLI and its saved
 * subscription session. The adapter never injects credentials and completes a
 * run only when the process exits cleanly, `system/init` was observed, and no
 * semantic failure event was streamed.
 *
 * @since 0.6.0
 * @param options - Executable override and bounded process/protocol limits.
 * @returns A Layer whose engine depends only on the shared process factory.
 */
export function make_claude_engine_layer(
	options: ClaudeEngineOptions = {},
): Layer.Layer<ClaudeEngine, never, EngineProcessFactory> {
	const configured = {
		...defaults,
		...options,
		executable: options.executable ?? defaults.executable,
		executable_args: options.executable_args ?? [],
	};
	return Layer.effect(
		ClaudeEngine,
		Effect.gen(function* () {
			const factory = yield* EngineProcessFactory;
			const Probe: Engine["Probe"] = () =>
				validate_positive(configured).pipe(Effect.andThen(make_probe(factory, configured)));
			const Open: Engine["Open"] = (input) =>
				Effect.gen(function* () {
					yield* validate_positive(configured);
					yield* validate_provider_options(input);
					const probe = yield* Probe({});
					if (!probe.ready)
						yield* Effect.fail(
							new EngineUnavailableError({
								engine_id: "claude",
								message:
									probe.authentication.reason ??
									"Claude authentication is not ready",
							}),
						);
					return yield* open_run(factory, configured, input);
				});
			const CheckNativeContinuation: NonNullable<Engine["CheckNativeContinuation"]> = (
				input,
			) =>
				Probe({}).pipe(
					Effect.map((probe) =>
						probe.version !== native_continuation_version
							? {
									reason: `Claude native resume is verified only for ${native_continuation_version}; found ${probe.version}`,
									state: "unsupported" as const,
								}
							: input.target_model === undefined
								? {
										reason: "Claude native continuation requires an explicit target model",
										state: "incompatible" as const,
									}
								: { state: "compatible" as const },
					),
				);
			const Usage: Required<Engine>["Usage"] = MakeClaudeUsage({
				...(configured.claude_config_dir === undefined
					? {}
					: { claude_config_dir: configured.claude_config_dir }),
				executable: configured.executable,
				executable_args: configured.executable_args,
				factory,
			});
			return {
				CheckNativeContinuation,
				Descriptor: ClaudeEngineDescriptor,
				Open,
				Probe,
				Usage,
			};
		}),
	);
}
