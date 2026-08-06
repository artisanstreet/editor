import { Buffer } from "node:buffer";
import { randomUUID } from "node:crypto";

import type {
	CanUseTool,
	Options,
	PermissionResult,
	Query,
	SDKMessage,
	SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";
import { Context, Effect, Encoding, Exit, Layer, Ref, Scope, Semaphore, Stream } from "effect";

import {
	type Engine,
	type EngineApprovalRequest,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineDescriptor,
	type EngineFailure,
	type EngineObservation,
	type EngineOpenInput,
	type EngineRun,
	type EngineUserInputPart,
	EngineCommandIdConflictError,
	EngineCommandTargetError,
	EngineConfigurationError,
	EngineProcessError,
	EngineProtocolError,
	EngineRunClosedError,
	EngineUnavailableError,
	EngineUnsupportedCommandError,
	EngineUnsupportedOperationError,
	ValidateEngineGlobalGuidance,
	ValidateEngineProductInstructions,
} from "../engine";
import { EngineProcessFactory } from "../process/process";
import { MakeEngineEventBuffer } from "../process/event-buffer";
import { WatchEngineInactivity } from "../process/inactivity-deadline";
import {
	claude_native_continuation_version,
	MakeClaudeProbe,
	type ClaudeProbeOptions,
} from "./probe";
import { MakeClaudeUsage } from "./usage";
import {
	classify_claude_semantic_failure,
	is_claude_init_event,
	normalize_claude_event,
	read_claude_session_id,
	read_claude_stream_message_id,
	read_claude_tool_uses,
	type ClaudeToolUse,
} from "./normalizer";
import { ClaudeQueryClient } from "./sdk-client";

const claude_transport = "claude-agent-sdk";
const claude_protocol_version = "claude-agent-sdk-v1";

/** Declares the Claude capability surface reachable through the Agent SDK. @since 0.8.0 */
export const ClaudeEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "supported",
			reason: "Native permission prompts bridge to canonical approvals through canUseTool.",
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
			reason: "Claude Code reads its own global CLAUDE.md natively; no mirror is wired.",
		},
		model_selection: {
			state: "supported",
			reason: "Native model identifiers pass through to the SDK's model option.",
		},
		native_continuation: {
			state: "supported",
			reason: `Same-engine model changes are explicitly target-model and Claude ${claude_native_continuation_version} gated.`,
		},
		native_tools: {
			state: "experimental",
			reason: "Known public tool activity is normalized; provider-native details remain raw.",
		},
		probe: { state: "supported", reason: "Only --version and auth status are probed." },
		question: {
			state: "unsupported",
			reason: "AskUserQuestion dialogs are not yet canonicalized.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported" },
		start: { state: "supported" },
		steer: {
			state: "experimental",
			reason: "Stream-input messages fold into the active turn; Claude Code owns the fold timing.",
		},
		subagents: {
			state: "experimental",
			reason: "Subagent activity streams through parent frames; not yet canonicalized.",
		},
	},
	display_name: "Claude",
	id: "claude",
	transport: claude_transport,
};

/** Configures the Claude engine's probe bounds and SDK runtime overrides. @since 0.8.0 */
export interface ClaudeEngineOptions {
	readonly auth_timeout_ms?: number;
	/** Overrides Claude's config-dir resolution used by `Usage` (normally env `CLAUDE_CONFIG_DIR`, else `~/.claude`). */
	readonly claude_config_dir?: string;
	readonly event_capacity?: number;
	/** The CLI probed for version and auth; the SDK runtime shares its credential store. */
	readonly executable?: string;
	readonly executable_args?: ReadonlyArray<string>;
	/** Silence that settles a run as stalled; every observation re-arms it. */
	readonly inactivity_ms?: number;
	readonly max_stderr_bytes?: number;
	readonly max_stdout_bytes?: number;
	/** Points the SDK at a specific Claude Code build instead of its bundled runtime. */
	readonly path_to_claude_code_executable?: string;
	readonly version_timeout_ms?: number;
}

interface ClaudeConfiguredOptions {
	readonly auth_timeout_ms: number;
	readonly event_capacity: number;
	readonly executable: string;
	readonly executable_args: ReadonlyArray<string>;
	readonly inactivity_ms: number;
	readonly max_stderr_bytes: number;
	readonly max_stdout_bytes: number;
	readonly path_to_claude_code_executable?: string;
	readonly version_timeout_ms: number;
}

/** Provides the Claude Code engine service. @since 0.6.0 */
export class ClaudeEngine extends Context.Service<ClaudeEngine, Engine>()("Artisan/ClaudeEngine") {}

/** Names every native permission mode accepted by Claude Code. @since 0.6.0 */
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
	/**
	 * Ten minutes of silence, not of running. A tool call can legitimately
	 * occupy the runtime without emitting anything, so the window has to clear
	 * the longest plausible quiet stretch; beyond it the stream is treated as
	 * dead rather than slow.
	 */
	inactivity_ms: 10 * 60 * 1_000,
	max_stderr_bytes: 1_048_576,
	max_stdout_bytes: 16 * 1_024 * 1_024,
	version_timeout_ms: 15_000,
	auth_timeout_ms: 15_000,
} as const;

function fail_configuration(option: string, value: unknown) {
	return Effect.fail(new EngineConfigurationError({ engine_id: "claude", option, value }));
}

function validate_positive(options: ClaudeEngineOptions) {
	const values = {
		event_capacity: options.event_capacity ?? defaults.event_capacity,
		inactivity_ms: options.inactivity_ms ?? defaults.inactivity_ms,
		max_stderr_bytes: options.max_stderr_bytes ?? defaults.max_stderr_bytes,
		max_stdout_bytes: options.max_stdout_bytes ?? defaults.max_stdout_bytes,
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

interface ClaudeResolvedRunOptions {
	readonly allow_dangerously_skip_permissions: boolean;
	readonly disable_tools: boolean;
	readonly permission_mode: (typeof claude_permission_modes)[number] | undefined;
	readonly product_instructions: string | undefined;
	readonly prompt_file: string | undefined;
	readonly safe_mode: boolean;
	/** Whether Artisan should answer native permission prompts through canUseTool. */
	readonly wants_approvals: boolean;
}

/**
 * Validates the caller's run context against what the SDK natively accepts.
 * The canonical permission policy maps onto Claude Code's prompt-driven
 * model: approvals are enforced by prompting, never by a sandbox, so policy
 * fields that promise filesystem or network isolation are rejected instead of
 * silently ignored.
 */
function validate_provider_options(
	input: EngineOpenInput,
): Effect.Effect<ClaudeResolvedRunOptions, EngineFailure> {
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

	const policy = input.permission_policy;

	if (policy !== undefined && permission_mode !== undefined) {
		return fail_configuration("provider_options.claude.permission_mode", permission_mode);
	}

	return Effect.gen(function* () {
		yield* ValidateEngineGlobalGuidance("claude", input.global_guidance);
		yield* ValidateEngineProductInstructions("claude", input.product_instructions);
		if (input.global_guidance !== undefined) {
			return yield* Effect.fail(
				new EngineUnsupportedOperationError({
					engine_id: "claude",
					operation: "global_guidance",
				}),
			);
		}

		if (policy === undefined) {
			return {
				allow_dangerously_skip_permissions: false,
				disable_tools: disable_tools === true,
				permission_mode: permission_mode as ClaudeResolvedRunOptions["permission_mode"],
				product_instructions: input.product_instructions?.content,
				prompt_file,
				safe_mode: safe_mode === true,
				wants_approvals: false,
			} satisfies ClaudeResolvedRunOptions;
		}

		if (!new Set(["never", "on_request", "always"]).has(policy.approval)) {
			return yield* fail_configuration("permission_policy.approval", policy.approval);
		}
		/** Claude Code has no sandbox on this transport; isolation promises would be lies. */
		if (policy.write_access !== true) {
			return yield* fail_configuration("permission_policy.write_access", policy.write_access);
		}
		if (policy.network_access !== true) {
			return yield* fail_configuration(
				"permission_policy.network_access",
				policy.network_access,
			);
		}
		if (
			policy.edit_scope !== undefined &&
			!new Set(["workspace", "host"]).has(policy.edit_scope)
		) {
			return yield* fail_configuration("permission_policy.edit_scope", policy.edit_scope);
		}

		return {
			allow_dangerously_skip_permissions: policy.approval === "never",
			disable_tools: disable_tools === true,
			permission_mode: policy.approval === "never" ? "bypassPermissions" : "default",
			product_instructions: input.product_instructions?.content,
			prompt_file,
			safe_mode: safe_mode === true,
			wants_approvals: policy.approval !== "never",
		} satisfies ClaudeResolvedRunOptions;
	});
}

/**
 * Serializes the user turn as the Anthropic message content the SDK's
 * stream-input messages accept. Text-only turns stay a plain string exactly
 * as before; ordered text/image parts become native content blocks.
 */
function make_user_content(text: string, content: ReadonlyArray<EngineUserInputPart> | undefined) {
	return content === undefined
		? text
		: content.map((part) =>
				part.type === "text"
					? { text: part.text, type: "text" as const }
					: {
							source: {
								data: Encoding.encodeBase64(part.bytes),
								media_type: part.media_type,
								type: "base64" as const,
							},
							type: "image" as const,
						},
			);
}

function make_user_message(
	session_id: string,
	text: string,
	content: ReadonlyArray<EngineUserInputPart> | undefined,
): SDKUserMessage {
	return {
		message: {
			content: make_user_content(text, content),
			role: "user",
		},
		parent_tool_use_id: null,
		session_id,
		type: "user",
	};
}

function make_diagnostic(
	input: EngineOpenInput,
	message: string,
	frame: unknown,
	level: "info" | "warning" | "error" = "info",
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
			protocol_version: claude_protocol_version,
			transport: claude_transport,
		},
		sequence: 0,
	};
}

/**
 * A single-consumer push channel bridging SDK callback land into async
 * iteration. The SDK owns the consuming generator (stream input) or the
 * engine owns it (approvals, stderr); producers stay plain synchronous
 * functions so SDK callbacks never need an Effect runtime.
 */
interface PushChannel<A> {
	readonly end: () => void;
	readonly iterable: AsyncIterable<A>;
	readonly push: (value: A) => void;
}

function make_push_channel<A>(): PushChannel<A> {
	const values: Array<A> = [];
	let ended = false;
	let notify: (() => void) | undefined;
	const wake = () => {
		const current = notify;
		notify = undefined;
		current?.();
	};

	return {
		end() {
			ended = true;
			wake();
		},
		iterable: {
			async *[Symbol.asyncIterator]() {
				while (true) {
					while (values.length > 0) yield values.shift() as A;
					if (ended) return;
					await new Promise<void>((resolve) => {
						notify = resolve;
					});
				}
			},
		},
		push(value) {
			if (ended) return;
			values.push(value);
			wake();
		},
	};
}

interface ClaudeApprovalAnnouncement {
	readonly approval_id: string;
	readonly description: string;
	readonly request: EngineApprovalRequest;
	readonly state: "requested" | "withdrawn";
}

function make_approval_request(
	tool_name: string,
	tool_input: Record<string, unknown>,
	reason: string | undefined,
): EngineApprovalRequest {
	if (tool_name === "Bash") {
		const command = tool_input["command"];

		return {
			kind: "command",
			...(typeof command === "string" ? { command } : {}),
			...(reason === undefined ? {} : { reason }),
		};
	}
	if (["Edit", "MultiEdit", "NotebookEdit", "Write"].includes(tool_name)) {
		return { kind: "file_change", ...(reason === undefined ? {} : { reason }) };
	}

	return { kind: "action", ...(reason === undefined ? {} : { reason }) };
}

function open_run(
	client: typeof ClaudeQueryClient.Service,
	options: ClaudeConfiguredOptions,
	input: EngineOpenInput,
): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> {
	return Effect.gen(function* () {
		const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
			Scope.close(scope, Exit.succeed(undefined)),
		);
		const resolved = yield* validate_provider_options(input);
		const native_session_id =
			input._tag === "resume" ? input.resume_token.native_thread_id : randomUUID();

		const abort = new AbortController();
		const input_channel = make_push_channel<SDKUserMessage>();
		const approval_channel = make_push_channel<ClaudeApprovalAnnouncement>();
		const stderr_channel = make_push_channel<string>();
		const pending_approvals = new Map<string, (result: PermissionResult) => void>();
		let approval_counter = 0;

		const can_use_tool: CanUseTool = (tool_name, tool_input, context) => {
			approval_counter += 1;
			const approval_id = `claude:${input.artisan_run_id}:approval:${approval_counter}`;
			const request = make_approval_request(tool_name, tool_input, context.decisionReason);
			const description = context.title ?? `Claude Code requests permission for ${tool_name}`;

			return new Promise<PermissionResult>((resolve) => {
				pending_approvals.set(approval_id, resolve);
				context.signal.addEventListener("abort", () => {
					if (pending_approvals.delete(approval_id)) {
						resolve({
							behavior: "deny",
							message: "Claude Code withdrew the permission request",
						});
						approval_channel.push({
							approval_id,
							description,
							request,
							state: "withdrawn",
						});
					}
				});
				approval_channel.push({ approval_id, description, request, state: "requested" });
			});
		};

		const query_options: Options = {
			abortController: abort,
			cwd: input.working_directory,
			env: { ...process.env },
			includePartialMessages: true,
			/**
			 * The SDK's default system prompt is minimal; the CLI's default is
			 * Claude Code's own. Selecting the preset keeps run behavior aligned
			 * with what the previous stream-json transport produced.
			 */
			systemPrompt: {
				type: "preset",
				preset: "claude_code",
				...(resolved.product_instructions === undefined
					? {}
					: { append: resolved.product_instructions }),
			},
			...(resolved.wants_approvals ? { canUseTool: can_use_tool } : {}),
			...(resolved.allow_dangerously_skip_permissions
				? { allowDangerouslySkipPermissions: true }
				: {}),
			...(resolved.permission_mode === undefined
				? {}
				: { permissionMode: resolved.permission_mode }),
			...(resolved.disable_tools ? { tools: [] } : {}),
			...(resolved.safe_mode || resolved.prompt_file !== undefined
				? {
						extraArgs: {
							...(resolved.safe_mode ? { "safe-mode": null } : {}),
							...(resolved.prompt_file === undefined
								? {}
								: { "append-system-prompt-file": resolved.prompt_file }),
						},
					}
				: {}),
			...(input.model === undefined ? {} : { model: input.model }),
			...(input._tag === "resume"
				? { resume: input.resume_token.native_thread_id }
				: { sessionId: native_session_id }),
			...(options.path_to_claude_code_executable === undefined
				? {}
				: { pathToClaudeCodeExecutable: options.path_to_claude_code_executable }),
			stderr: (data) => stderr_channel.push(data),
		};

		const prompt = input._tag === "start" ? input.initial_text : input.next_text;
		const content = input._tag === "start" ? input.initial_content : input.next_content;

		if (prompt !== undefined || (content !== undefined && content.length > 0)) {
			input_channel.push(make_user_message(native_session_id, prompt ?? "", content));
		}

		let query_handle: Query | undefined;
		const close_query = Effect.sync(() => {
			for (const resolve of pending_approvals.values()) {
				resolve({ behavior: "deny", message: "The Artisan run is closing" });
			}
			pending_approvals.clear();
			input_channel.end();
			approval_channel.end();
			stderr_channel.end();
			abort.abort();
			query_handle?.close();
		});

		const event_buffer = yield* MakeEngineEventBuffer({
			artisan_run_id: input.artisan_run_id,
			capacity: options.event_capacity,
			CloseResource: close_query,
			make_terminal_observation: (state, sequence) => ({
				_tag: "run_terminal",
				artisan_run_id: input.artisan_run_id,
				observation_id: `${input.artisan_run_id}:claude:terminal`,
				raw: {
					engine_id: "claude",
					frame: { state },
					protocol_version: claude_protocol_version,
					transport: claude_transport,
				},
				sequence,
				state,
			}),
		});
		const state = yield* Ref.make({
			frame_sequence: 0,
			semantic_failure: false,
			command_intents: new Map<string, string>(),
			session_id: native_session_id,
			init_seen: false,
			result_seen: false,
			/** The message id announced by the newest `message_start` frame. */
			stream_message_id: undefined as string | undefined,
			/**
			 * Native tool uses are run-owned stream state. Claude emits results in
			 * later user frames with only a tool-use id, so this maps those results
			 * back onto the exact canonical family their assistant frame started.
			 */
			tool_uses: new Map<string, ClaudeToolUse>() as ReadonlyMap<string, ClaudeToolUse>,
		});

		query_handle = client.Start({ options: query_options, prompt: input_channel.iterable });

		const process_message = (message: SDKMessage) =>
			Effect.gen(function* () {
				const frame_sequence = yield* Ref.modify(
					state,
					(current) =>
						[
							current.frame_sequence + 1,
							{ ...current, frame_sequence: current.frame_sequence + 1 },
						] as const,
				);
				const session_id = read_claude_session_id(message);
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
				if (is_claude_init_event(message))
					yield* Ref.update(state, (current) => ({ ...current, init_seen: true }));
				if (classify_claude_semantic_failure(message))
					yield* Ref.update(state, (current) => ({ ...current, semantic_failure: true }));
				/**
				 * `message_start` is the only frame that names the message its
				 * deltas belong to, so the id is carried forward until the next
				 * message begins. Without it the completion would land on a
				 * second conversation item beside the streamed one.
				 */
				const announced_message_id = read_claude_stream_message_id(message);
				const stream_message_id = announced_message_id ?? current_state.stream_message_id;
				const announced_tools = read_claude_tool_uses(message);
				const tool_uses: ReadonlyMap<string, ClaudeToolUse> =
					announced_tools.length === 0
						? current_state.tool_uses
						: new Map<string, ClaudeToolUse>([
								...current_state.tool_uses,
								...announced_tools.map((tool): readonly [string, ClaudeToolUse] => [
									tool.id,
									tool,
								]),
							]);
				if (announced_message_id !== undefined)
					yield* Ref.update(state, (current) => ({
						...current,
						stream_message_id: announced_message_id,
					}));
				if (announced_tools.length > 0)
					yield* Ref.update(state, (current) => ({ ...current, tool_uses }));
				yield* Effect.forEach(
					normalize_claude_event({
						artisan_run_id: input.artisan_run_id,
						frame_sequence,
						payload: message,
						protocol_version: claude_protocol_version,
						raw_frame_base64: Encoding.encodeBase64(
							Buffer.from(JSON.stringify(message), "utf8"),
						),
						...(stream_message_id === undefined ? {} : { stream_message_id }),
						tool_uses,
						transport: claude_transport,
						turn_id: `claude:${input.artisan_run_id}:turn`,
					}),
					event_buffer.Emit,
				);
				/**
				 * The stream-input session stays alive after a turn, so the result
				 * frame — not child exit — is the turn's terminal signal.
				 */
				if (message.type === "result") {
					yield* Ref.update(state, (current) => ({ ...current, result_seen: true }));
					const settled = yield* Ref.get(state);
					yield* event_buffer.Finish(
						settled.init_seen && !settled.semantic_failure ? "completed" : "failed",
					);
				}
			});

		const pump_messages = Stream.fromAsyncIterable(
			query_handle,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach(process_message),
			Effect.andThen(
				Effect.gen(function* () {
					const settled = yield* Ref.get(state);
					if (!settled.result_seen) {
						yield* event_buffer
							.Emit(
								make_diagnostic(
									input,
									"Claude stream ended without a result frame",
									{ init_seen: settled.init_seen },
									"error",
								),
							)
							.pipe(Effect.ignore);
						yield* event_buffer.Finish("failed");
					}
				}),
			),
			Effect.catch((error) =>
				event_buffer
					.Emit(make_diagnostic(input, String(error), { source: "sdk-stream" }, "error"))
					.pipe(Effect.ignore, Effect.andThen(event_buffer.Finish("failed"))),
			),
		);
		const pump_approvals = Stream.fromAsyncIterable(
			approval_channel.iterable,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((announcement) =>
				event_buffer.Emit({
					_tag: "approval",
					approval_id: announcement.approval_id,
					...(announcement.state === "withdrawn" ? { approved: false } : {}),
					artisan_run_id: input.artisan_run_id,
					description: announcement.description,
					observation_id: `${input.artisan_run_id}:claude:${announcement.approval_id}:${announcement.state}`,
					raw: {
						engine_id: "claude",
						frame: announcement,
						protocol_version: claude_protocol_version,
						transport: claude_transport,
					},
					request: announcement.request,
					sequence: 0,
					state: announcement.state === "requested" ? "requested" : "resolved",
				}),
			),
			Effect.ignore,
		);
		const pump_stderr = Stream.fromAsyncIterable(
			stderr_channel.iterable,
			(cause) => new EngineProcessError({ cause, operation: "read" }),
		).pipe(
			Stream.runForEach((line) =>
				event_buffer.Emit(make_diagnostic(input, line, { source: "stderr" }, "info")),
			),
			Effect.ignore,
		);
		const watch_inactivity = WatchEngineInactivity({
			Activity: event_buffer.Activity,
			Closed: event_buffer.Closed,
			/** The session is opened per run, so it owes output for the whole run. */
			Expecting: Effect.succeed(true),
			inactivity_ms: options.inactivity_ms,
			OnStall: event_buffer
				.Emit(
					make_diagnostic(
						input,
						`Claude produced no output for ${options.inactivity_ms}ms and is treated as stalled`,
						{ inactivity_ms: options.inactivity_ms },
						"error",
					),
				)
				.pipe(Effect.andThen(event_buffer.Finish("failed"))),
		});
		yield* Effect.forkScoped(pump_messages).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(pump_approvals).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(pump_stderr).pipe(Scope.provide(run_scope));
		yield* Effect.forkScoped(watch_inactivity).pipe(Scope.provide(run_scope));
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
					const remember = Ref.update(state, (value) => ({
						...value,
						command_intents: new Map(value.command_intents).set(
							command.command_id,
							intent,
						),
					}));

					if (command._tag === "cancel" || command._tag === "close") {
						yield* remember;
						return yield* event_buffer.Finish(
							command._tag === "cancel" ? "cancelled" : "closed",
						);
					}
					if (command._tag === "steer") {
						yield* remember;
						return yield* Effect.sync(() =>
							input_channel.push(
								make_user_message(
									native_session_id,
									command.text,
									command.content ?? undefined,
								),
							),
						);
					}
					if (command._tag === "respond_approval") {
						const resolve = pending_approvals.get(command.approval_id);
						if (resolve === undefined) {
							return yield* Effect.fail(
								new EngineCommandTargetError({
									artisan_run_id: input.artisan_run_id,
									command_id: command.command_id,
									target: "approval",
									target_id: command.approval_id,
								}),
							);
						}
						yield* remember;
						yield* event_buffer.Emit({
							_tag: "approval",
							approval_id: command.approval_id,
							approved: command.approved,
							artisan_run_id: input.artisan_run_id,
							description: "Artisan resolved the permission request",
							observation_id: `${input.artisan_run_id}:claude:${command.approval_id}:resolved`,
							raw: {
								engine_id: "claude",
								frame: {
									approved: command.approved,
									command_id: command.command_id,
								},
								protocol_version: claude_protocol_version,
								transport: claude_transport,
							},
							request: { kind: "action" },
							sequence: 0,
							state: "resolved",
						});
						pending_approvals.delete(command.approval_id);
						return yield* Effect.sync(() =>
							resolve(
								command.approved
									? { behavior: "allow" }
									: {
											behavior: "deny",
											message: "The Artisan user denied this action",
										},
							),
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
			native_thread_id: native_session_id,
			resume_token: { native_thread_id: native_session_id },
			Send,
		} satisfies EngineRun;
	});
}

/**
 * Builds a Claude engine on the Agent SDK's managed Claude Code runtime with
 * the CLI's saved subscription session. The adapter never injects
 * credentials; a run completes only when the session streamed `system/init`,
 * finished its turn with a result frame, and reported no semantic failure.
 *
 * @since 0.8.0
 * @param options - Probe bounds and SDK runtime overrides.
 * @returns A Layer requiring the shared process factory and the query client.
 */
export function make_claude_engine_layer(
	options: ClaudeEngineOptions = {},
): Layer.Layer<ClaudeEngine, never, EngineProcessFactory | ClaudeQueryClient> {
	const configured: ClaudeConfiguredOptions & ClaudeEngineOptions = {
		...defaults,
		...options,
		executable: options.executable ?? defaults.executable,
		executable_args: options.executable_args ?? [],
	};
	return Layer.effect(
		ClaudeEngine,
		Effect.gen(function* () {
			const factory = yield* EngineProcessFactory;
			const client = yield* ClaudeQueryClient;
			const probe_options: ClaudeProbeOptions = {
				auth_timeout_ms: configured.auth_timeout_ms,
				descriptor: ClaudeEngineDescriptor,
				executable: configured.executable,
				executable_args: configured.executable_args,
				max_stderr_bytes: configured.max_stderr_bytes,
				max_stdout_bytes: configured.max_stdout_bytes,
				version_timeout_ms: configured.version_timeout_ms,
			};
			const Probe: Engine["Probe"] = () =>
				validate_positive(configured).pipe(
					Effect.andThen(MakeClaudeProbe(factory, probe_options)),
				);
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
					return yield* open_run(client, configured, input);
				});
			const CheckNativeContinuation: NonNullable<Engine["CheckNativeContinuation"]> = (
				input,
			) =>
				Probe({}).pipe(
					Effect.map((probe) =>
						probe.version !== claude_native_continuation_version
							? {
									reason: `Claude native resume is verified only for ${claude_native_continuation_version}; found ${probe.version}`,
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
