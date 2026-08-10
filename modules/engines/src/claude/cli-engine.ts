import { Buffer } from "node:buffer";
import { randomUUID } from "node:crypto";

import {
	Context,
	Deferred,
	Effect,
	Encoding,
	Exit,
	Layer,
	Option,
	Ref,
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	type Engine,
	type EngineApprovalRequest,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineFailure,
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
import { MakeEngineEventBuffer } from "../process/event-buffer";
import { WatchEngineInactivity } from "../process/inactivity-deadline";
import { EngineProcessFactory } from "../process/process";
import { ClaudeEngineDescriptor, claude_protocol_version, claude_transport } from "./descriptor";
import {
	AdvanceClaudeChildTranscripts,
	EmptyClaudeChildTranscripts,
	is_claude_child_transcript_frame,
	MakeClaudeChildTranscriptBase,
} from "./child-transcripts";
import { claude_artisan_error_codes } from "./errors";
import {
	ClaudeJsonlFramer,
	ClaudeJsonlMalformedLineError,
	ClaudeJsonlOversizedLineError,
} from "./jsonl";
import {
	normalize_claude_event,
	classify_claude_semantic_failure,
	is_claude_init_event,
	has_claude_subagent_lifecycle_hint,
	read_claude_session_id,
	read_claude_stream_message_id,
	read_claude_task_id,
	read_claude_task_started,
	read_claude_tool_uses,
} from "./normalizer";
import { claude_native_continuation_version, MakeClaudeProbe } from "./probe";
import { AdvanceClaudeTaskLineage, EmptyClaudeTaskLineage } from "./task-lineage";
import { MakeClaudeUsage } from "./usage";

/** Provides Claude through the user's external Claude Code CLI. */
export class ClaudeEngine extends Context.Service<ClaudeEngine, Engine>()("Artisan/ClaudeEngine") {}

export interface ClaudeEngineOptions {
	readonly executable?: string;
	readonly executable_args?: ReadonlyArray<string>;
	readonly auth_retry_attempts?: number;
	readonly auth_retry_delay_ms?: number;
	readonly auth_timeout_ms?: number;
	readonly version_timeout_ms?: number;
	readonly event_capacity?: number;
	readonly inactivity_ms?: number;
	readonly max_frame_bytes?: number;
	readonly max_stdout_bytes?: number;
	readonly max_stderr_bytes?: number;
}

const defaults = {
	executable: "claude",
	executable_args: [] as ReadonlyArray<string>,
	auth_retry_attempts: 2,
	auth_retry_delay_ms: 1_000,
	auth_timeout_ms: 15_000,
	version_timeout_ms: 15_000,
	event_capacity: 256,
	inactivity_ms: 600_000,
	max_frame_bytes: 1_048_576,
	max_stdout_bytes: 16 * 1024 * 1024,
	max_stderr_bytes: 1_048_576,
};
const ControlPermissionRequest = Schema.Struct({
	type: Schema.Literal("control_request"),
	request_id: Schema.String,
	request: Schema.Struct({
		subtype: Schema.Literal("can_use_tool"),
		tool_name: Schema.String,
		input: Schema.Record(Schema.String, Schema.Unknown),
		title: Schema.optional(Schema.String),
		description: Schema.optional(Schema.String),
	}),
});

const ToUserMessage = (
	session_id: string,
	text: string,
	content: ReadonlyArray<EngineUserInputPart> | undefined,
) => ({
	message: {
		content:
			content === undefined
				? text
				: content.map((part) =>
						part.type === "text"
							? { type: "text", text: part.text }
							: {
									type: "image",
									source: {
										type: "base64",
										media_type: part.media_type,
										data: Encoding.encodeBase64(part.bytes),
									},
								},
					),
		role: "user",
	},
	parent_tool_use_id: null,
	session_id,
	type: "user",
});

const ReadApproval = (
	payload: unknown,
):
	| { readonly id: string; readonly request: EngineApprovalRequest; readonly description: string }
	| undefined => {
	const decoded = Schema.decodeUnknownOption(ControlPermissionRequest, {
		onExcessProperty: "preserve",
	})(payload);
	if (Option.isNone(decoded)) return undefined;
	const request = decoded.value.request;
	return {
		id: decoded.value.request_id,
		description:
			request.title ??
			request.description ??
			`Claude requests permission for ${request.tool_name}`,
		request:
			request.tool_name === "Bash"
				? {
						kind: "command",
						...(typeof request.input.command === "string"
							? { command: request.input.command }
							: {}),
					}
				: ["Edit", "Write", "MultiEdit", "NotebookEdit"].includes(request.tool_name)
					? { kind: "file_change" }
					: { kind: "action" },
	};
};

/**
 * Uses Claude Code's stdio permission control protocol. Both directions are
 * schema-shaped at this boundary and covered by captured CLI process tests.
 */
const ApprovalResponse = (request_id: string, approved: boolean) => ({
	type: "control_response" as const,
	response: {
		subtype: "success" as const,
		request_id,
		response: { behavior: approved ? ("allow" as const) : ("deny" as const) },
	},
});

/** Native permission modes accepted across supported Claude CLI generations. */
export const claude_permission_modes = [
	"plan",
	"default",
	"acceptEdits",
	"auto",
	"bypassPermissions",
	"manual",
	"dontAsk",
] as const;

interface ClaudeResolvedRunOptions {
	readonly dangerous_bypass: boolean;
	readonly disable_tools: boolean;
	readonly permission_mode: (typeof claude_permission_modes)[number] | undefined;
	readonly product_instructions: string | undefined;
	readonly prompt_file: string | undefined;
	readonly safe_mode: boolean;
}

const FailConfiguration = (option: string, value: unknown) =>
	Effect.fail(new EngineConfigurationError({ engine_id: "claude", option, value }));

const ValidateConfiguration = (options: typeof defaults) => {
	const positive = {
		auth_retry_delay_ms: options.auth_retry_delay_ms,
		auth_timeout_ms: options.auth_timeout_ms,
		event_capacity: options.event_capacity,
		inactivity_ms: options.inactivity_ms,
		max_frame_bytes: options.max_frame_bytes,
		max_stderr_bytes: options.max_stderr_bytes,
		max_stdout_bytes: options.max_stdout_bytes,
		version_timeout_ms: options.version_timeout_ms,
	};
	const invalid = Object.entries(positive).find(
		([, value]) => !Number.isSafeInteger(value) || value <= 0,
	);
	if (invalid !== undefined) return FailConfiguration(invalid[0], invalid[1]);
	return !Number.isSafeInteger(options.auth_retry_attempts) || options.auth_retry_attempts < 0
		? FailConfiguration("auth_retry_attempts", options.auth_retry_attempts)
		: Effect.void;
};

/** Resolves canonical policy and provider-specific switches to truthful CLI flags. */
const ResolveRunOptions = (
	input: EngineOpenInput,
): Effect.Effect<ClaudeResolvedRunOptions, EngineFailure> => {
	const options = input.provider_options ?? {};
	const allowed = new Set([
		"claude.permission_mode",
		"claude.append_system_prompt_file",
		"claude.disable_tools",
		"claude.safe_mode",
	]);
	const unknown = Object.keys(options).find((key) => !allowed.has(key));
	if (unknown !== undefined)
		return FailConfiguration(`provider_options.${unknown}`, options[unknown]);

	const permission_mode = options["claude.permission_mode"];
	if (
		permission_mode !== undefined &&
		(typeof permission_mode !== "string" ||
			!(claude_permission_modes as ReadonlyArray<string>).includes(permission_mode))
	)
		return FailConfiguration("provider_options.claude.permission_mode", permission_mode);
	const prompt_file = options["claude.append_system_prompt_file"];
	if (
		prompt_file !== undefined &&
		(typeof prompt_file !== "string" || prompt_file.trim().length === 0)
	)
		return FailConfiguration("provider_options.claude.append_system_prompt_file", prompt_file);
	const disable_tools = options["claude.disable_tools"];
	if (disable_tools !== undefined && disable_tools !== true)
		return FailConfiguration("provider_options.claude.disable_tools", disable_tools);
	const safe_mode = options["claude.safe_mode"];
	if (safe_mode !== undefined && safe_mode !== true)
		return FailConfiguration("provider_options.claude.safe_mode", safe_mode);

	const policy = input.permission_policy;
	if (policy !== undefined && permission_mode !== undefined)
		return FailConfiguration("provider_options.claude.permission_mode", permission_mode);

	return Effect.gen(function* () {
		yield* ValidateEngineGlobalGuidance("claude", input.global_guidance);
		yield* ValidateEngineProductInstructions("claude", input.product_instructions);
		if (input.global_guidance !== undefined)
			return yield* Effect.fail(
				new EngineUnsupportedOperationError({
					engine_id: "claude",
					operation: "global_guidance",
				}),
			);

		if (policy !== undefined) {
			if (!new Set(["never", "on_request", "always"]).has(policy.approval))
				return yield* FailConfiguration("permission_policy.approval", policy.approval);
			if (policy.write_access !== true)
				return yield* FailConfiguration(
					"permission_policy.write_access",
					policy.write_access,
				);
			if (policy.network_access !== true)
				return yield* FailConfiguration(
					"permission_policy.network_access",
					policy.network_access,
				);
			if (
				policy.edit_scope !== undefined &&
				!new Set(["workspace", "host"]).has(policy.edit_scope)
			)
				return yield* FailConfiguration("permission_policy.edit_scope", policy.edit_scope);
			if (policy.approval === "never" && policy.edit_scope !== "host")
				return yield* FailConfiguration("permission_policy.edit_scope", policy.edit_scope);
		}

		const resolved_mode = permission_mode as ClaudeResolvedRunOptions["permission_mode"];
		return {
			dangerous_bypass: policy?.approval === "never" || resolved_mode === "bypassPermissions",
			disable_tools: disable_tools === true,
			permission_mode:
				policy !== undefined ||
				resolved_mode === "default" ||
				resolved_mode === "bypassPermissions"
					? undefined
					: resolved_mode,
			product_instructions: input.product_instructions?.content,
			prompt_file: prompt_file as string | undefined,
			safe_mode: safe_mode === true,
		} satisfies ClaudeResolvedRunOptions;
	});
};

export const make_claude_engine_layer = (
	input: ClaudeEngineOptions = {},
): Layer.Layer<ClaudeEngine, never, EngineProcessFactory> => {
	const options = {
		auth_retry_attempts: input.auth_retry_attempts ?? defaults.auth_retry_attempts,
		auth_retry_delay_ms: input.auth_retry_delay_ms ?? defaults.auth_retry_delay_ms,
		auth_timeout_ms: input.auth_timeout_ms ?? defaults.auth_timeout_ms,
		executable: input.executable ?? defaults.executable,
		executable_args: input.executable_args ?? defaults.executable_args,
		event_capacity: input.event_capacity ?? defaults.event_capacity,
		inactivity_ms: input.inactivity_ms ?? defaults.inactivity_ms,
		max_frame_bytes: input.max_frame_bytes ?? defaults.max_frame_bytes,
		max_stderr_bytes: input.max_stderr_bytes ?? defaults.max_stderr_bytes,
		max_stdout_bytes: input.max_stdout_bytes ?? defaults.max_stdout_bytes,
		version_timeout_ms: input.version_timeout_ms ?? defaults.version_timeout_ms,
	};
	return Layer.effect(
		ClaudeEngine,
		Effect.gen(function* () {
			const factory = yield* EngineProcessFactory;
			const validated = () => ValidateConfiguration(options);
			const Probe: Engine["Probe"] = () =>
				validated().pipe(
					Effect.andThen(
						MakeClaudeProbe(factory, {
							auth_timeout_ms: options.auth_timeout_ms,
							descriptor: ClaudeEngineDescriptor,
							executable: options.executable,
							executable_args: options.executable_args,
							max_stderr_bytes: options.max_stderr_bytes,
							max_stdout_bytes: options.max_stdout_bytes,
							version_timeout_ms: options.version_timeout_ms,
						}),
					),
				);
			const Open: Engine["Open"] = (input) =>
				Effect.gen(function* () {
					yield* validated();
					const resolved = yield* ResolveRunOptions(input);
					let probe = yield* Probe({});
					for (
						let attempt = 0;
						!probe.ready && attempt < options.auth_retry_attempts;
						attempt += 1
					) {
						yield* Effect.sleep(options.auth_retry_delay_ms * 2 ** attempt);
						probe = yield* Probe({});
					}
					if (!probe.ready)
						return yield* Effect.fail(
							new EngineUnavailableError({
								artisan_code:
									probe.authentication.state === "unauthenticated"
										? claude_artisan_error_codes.engine_not_signed_in
										: claude_artisan_error_codes.provider_auth_failed,
								engine_id: "claude",
								message:
									probe.authentication.reason ??
									"Claude authentication is not ready",
							}),
						);
					const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
						Scope.close(scope, Exit.void),
					);
					const session_id =
						input._tag === "resume"
							? input.resume_token.native_thread_id
							: randomUUID();
					const args = [
						...options.executable_args,
						...(input._tag === "resume"
							? ["--resume", session_id]
							: ["--session-id", session_id]),
						"-p",
						"--output-format",
						"stream-json",
						"--input-format",
						"stream-json",
						"--verbose",
						"--include-partial-messages",
						"--forward-subagent-text",
						"--permission-prompt-tool",
						"stdio",
						...(resolved.dangerous_bypass
							? ["--dangerously-skip-permissions"]
							: resolved.permission_mode === undefined
								? []
								: ["--permission-mode", resolved.permission_mode]),
						...(resolved.disable_tools ? ["--tools", ""] : []),
						...(resolved.safe_mode ? ["--safe-mode"] : []),
						...(resolved.prompt_file === undefined
							? []
							: ["--append-system-prompt-file", resolved.prompt_file]),
						...(input.model === undefined ? [] : ["--model", input.model]),
						...(resolved.product_instructions === undefined
							? []
							: ["--append-system-prompt", resolved.product_instructions]),
					];
					const handle = yield* factory
						.Spawn({
							command: options.executable,
							args,
							cwd: input.working_directory,
							env: { ...process.env },
						})
						.pipe(Scope.provide(run_scope));
					yield* Scope.addFinalizer(run_scope, handle.Close);
					const first_text =
						input._tag === "start" ? input.initial_text : input.next_text;
					const first_content =
						input._tag === "start" ? input.initial_content : input.next_content;
					if (first_text !== undefined || (first_content?.length ?? 0) > 0)
						yield* handle.Write(
							new TextEncoder().encode(
								`${JSON.stringify(ToUserMessage(session_id, first_text ?? "", first_content))}\n`,
							),
						);
					const events = yield* MakeEngineEventBuffer({
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
								protocol_version: claude_protocol_version,
								transport: claude_transport,
							},
							sequence,
							state,
						}),
					});
					const state = yield* Ref.make({
						command_intents: new Map<string, string>(),
						frame_sequence: 0,
						init_seen: false,
						input_ended: false,
						pending: new Map<string, ReturnType<typeof ReadApproval>>(),
						child_transcripts: EmptyClaudeChildTranscripts(),
						result_seen: false,
						semantic_failure: false,
						stderr_bytes: 0,
						stream_message_id: undefined as string | undefined,
						stdout_bytes: 0,
						task_lineage: EmptyClaudeTaskLineage(),
						tool_uses: new Map<
							string,
							ReturnType<typeof read_claude_tool_uses>[number]
						>(),
					});
					const framer = new ClaudeJsonlFramer({
						max_frame_bytes: options.max_frame_bytes,
					});
					const Process = (
						decoded: ReturnType<ClaudeJsonlFramer["PushRecovering"]>[number],
					) =>
						Effect.gen(function* () {
							if (
								decoded instanceof ClaudeJsonlMalformedLineError ||
								decoded instanceof ClaudeJsonlOversizedLineError
							)
								return yield* Effect.fail(
									new EngineProtocolError({
										engine_id: "claude",
										message: decoded.message,
									}),
								);
							const frame = decoded.payload;
							const current = yield* Ref.get(state);
							const sequence = current.frame_sequence + 1;
							yield* Ref.update(state, (value) => ({
								...value,
								frame_sequence: sequence,
							}));
							if (
								is_claude_init_event(frame) ||
								classify_claude_semantic_failure(frame)
							)
								yield* Ref.update(state, (value) => ({
									...value,
									...(is_claude_init_event(frame) ? { init_seen: true } : {}),
									...(classify_claude_semantic_failure(frame)
										? { semantic_failure: true }
										: {}),
								}));
							const approval = ReadApproval(frame);
							if (approval !== undefined) {
								yield* Ref.update(state, (value) => ({
									...value,
									pending: new Map(value.pending).set(approval.id, approval),
								}));
								return yield* events.Emit({
									_tag: "approval",
									approval_id: approval.id,
									artisan_run_id: input.artisan_run_id,
									description: approval.description,
									observation_id: `${input.artisan_run_id}:claude:${approval.id}:requested`,
									raw: {
										engine_id: "claude",
										frame,
										frame_sequence: sequence,
										protocol_version: claude_protocol_version,
										raw_frame_base64: decoded.raw_frame_base64,
										transport: claude_transport,
									},
									request: approval.request,
									sequence: 0,
									state: "requested",
								});
							}
							const child_frame = is_claude_child_transcript_frame(frame);
							const native = read_claude_session_id(frame);
							if (!child_frame && native !== undefined && native !== session_id)
								return yield* Effect.fail(
									new EngineProtocolError({
										engine_id: "claude",
										message: "Claude session identity mismatch",
									}),
								);
							const announced_tools = read_claude_tool_uses(frame);
							const task_id = read_claude_task_id(frame);
							const task_started = read_claude_task_started(frame);
							const lineage = AdvanceClaudeTaskLineage(current.task_lineage, {
								announced_tools,
								...(child_frame
									? { child_parent_tool_use_id: frame.parent_tool_use_id }
									: {}),
								lifecycle_has_subagent_hint:
									has_claude_subagent_lifecycle_hint(frame),
								...(task_id === undefined ? {} : { lifecycle_task_id: task_id }),
								...(task_started === undefined ? {} : { task_started }),
								root_native_thread_id: session_id,
							});
							const stream_message_id =
								read_claude_stream_message_id(frame) ?? current.stream_message_id;
							yield* Ref.update(state, (value) => ({
								...value,
								task_lineage: lineage,
								stream_message_id,
								tool_uses: new Map([
									...value.tool_uses,
									...announced_tools.map((tool) => [tool.id, tool] as const),
								]),
							}));
							if (child_frame) {
								const child = AdvanceClaudeChildTranscripts({
									base: MakeClaudeChildTranscriptBase(
										input.artisan_run_id,
										frame,
										decoded.raw_frame_base64,
									),
									frame_sequence: sequence,
									lineage,
									message: frame,
									parent_tool_use_id: frame.parent_tool_use_id,
									state: current.child_transcripts,
								});
								yield* Ref.update(state, (value) => ({
									...value,
									child_transcripts: child.state,
								}));
								return yield* Effect.forEach(child.observations, events.Emit);
							}
							const flushed_children = AdvanceClaudeChildTranscripts({
								base: MakeClaudeChildTranscriptBase(
									input.artisan_run_id,
									frame,
									decoded.raw_frame_base64,
								),
								frame_sequence: sequence,
								lineage,
								state: current.child_transcripts,
							});
							if (flushed_children.observations.length > 0) {
								yield* Ref.update(state, (value) => ({
									...value,
									child_transcripts: flushed_children.state,
								}));
								yield* Effect.forEach(flushed_children.observations, events.Emit);
							}
							const lifecycle_parent_native_thread_id =
								task_id === undefined
									? undefined
									: lineage.parent_native_thread_by_task_id.get(task_id);
							yield* Effect.forEach(
								normalize_claude_event({
									artisan_run_id: input.artisan_run_id,
									frame_sequence: sequence,
									native_thread_id: session_id,
									payload: frame,
									protocol_version: claude_protocol_version,
									raw_frame_base64: decoded.raw_frame_base64,
									...(stream_message_id === undefined
										? {}
										: { stream_message_id }),
									...(task_id !== undefined &&
									lineage.subagent_task_ids.has(task_id)
										? { native_subagent_task: true }
										: {}),
									...(lifecycle_parent_native_thread_id === undefined
										? {}
										: {
												subagent_parent_native_thread_id:
													lifecycle_parent_native_thread_id,
											}),
									tool_uses: new Map([
										...current.tool_uses,
										...announced_tools.map((tool) => [tool.id, tool] as const),
									]),
									transport: claude_transport,
									turn_id: `claude:${input.artisan_run_id}:turn`,
								}),
								events.Emit,
							);
							const result =
								frame !== null &&
								typeof frame === "object" &&
								"type" in frame &&
								frame.type === "result";
							if (result) {
								const prior = yield* Ref.get(state);
								yield* Ref.update(state, (value) => ({
									...value,
									input_ended: true,
									result_seen: true,
								}));
								if (!prior.input_ended) yield* handle.EndInput;
							}
						});
					const stdout_drained = yield* Deferred.make<void>();
					const stderr_drained = yield* Deferred.make<void>();
					const stdout = Stream.fromAsyncIterable(
						handle.Stdout,
						(cause) => new EngineProcessError({ cause, operation: "read" }),
					).pipe(
						Stream.runForEach((chunk) =>
							Effect.gen(function* () {
								const current = yield* Ref.updateAndGet(state, (value) => ({
									...value,
									stdout_bytes: value.stdout_bytes + chunk.byteLength,
								}));
								if (current.stdout_bytes > options.max_stdout_bytes)
									return yield* Effect.fail(
										new EngineProtocolError({
											engine_id: "claude",
											message: "Claude stdout limit exceeded",
										}),
									);
								return yield* Effect.forEach(framer.PushRecovering(chunk), Process);
							}),
						),
						Effect.andThen(Effect.forEach(framer.FinishRecovering(), Process)),
						Effect.ensuring(Deferred.succeed(stdout_drained, undefined)),
						Effect.catch((error) =>
							events
								.Finish("failed")
								.pipe(Effect.andThen(Effect.logWarning(String(error)))),
						),
					);
					const stderr = Stream.fromAsyncIterable(
						handle.Stderr,
						(cause) => new EngineProcessError({ cause, operation: "read" }),
					).pipe(
						Stream.runForEach((chunk) =>
							Effect.gen(function* () {
								const current = yield* Ref.updateAndGet(state, (value) => ({
									...value,
									stderr_bytes: value.stderr_bytes + chunk.byteLength,
								}));
								if (current.stderr_bytes > options.max_stderr_bytes)
									return yield* Effect.fail(
										new EngineProtocolError({
											engine_id: "claude",
											message: "Claude stderr limit exceeded",
										}),
									);
								yield* events.Emit({
									_tag: "process_diagnostic",
									artisan_run_id: input.artisan_run_id,
									level: "info",
									message: new TextDecoder().decode(chunk),
									observation_id: `${input.artisan_run_id}:claude:stderr`,
									raw: {
										engine_id: "claude",
										frame: { source: "stderr" },
										protocol_version: claude_protocol_version,
										raw_frame_base64: Buffer.from(chunk).toString("base64"),
										transport: claude_transport,
									},
									sequence: 0,
								});
							}),
						),
						Effect.ensuring(Deferred.succeed(stderr_drained, undefined)),
						Effect.catch(() => events.Finish("failed")),
					);
					const exit = handle.Exit.pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								yield* Deferred.await(stdout_drained);
								yield* Deferred.await(stderr_drained);
								const current = yield* Ref.get(state);
								return yield* events.Finish(
									result.code === 0 &&
										current.init_seen &&
										current.result_seen &&
										!current.semantic_failure
										? "completed"
										: "failed",
								);
							}),
						),
						Effect.catch(() => events.Finish("failed")),
					);
					const inactivity = WatchEngineInactivity({
						Activity: events.Activity,
						Closed: events.Closed,
						Expecting: Effect.succeed(true),
						inactivity_ms: options.inactivity_ms,
						OnStall: events
							.Emit({
								_tag: "process_diagnostic",
								artisan_run_id: input.artisan_run_id,
								level: "error",
								message: `Claude produced no output for ${options.inactivity_ms}ms and is treated as stalled`,
								observation_id: `${input.artisan_run_id}:claude:stalled`,
								raw: {
									engine_id: "claude",
									frame: { inactivity_ms: options.inactivity_ms },
									protocol_version: claude_protocol_version,
									transport: claude_transport,
								},
								sequence: 0,
							})
							.pipe(Effect.andThen(events.Finish("failed"))),
					});
					yield* Effect.forkScoped(stdout).pipe(Scope.provide(run_scope));
					yield* Effect.forkScoped(stderr).pipe(Scope.provide(run_scope));
					yield* Effect.forkScoped(exit).pipe(Scope.provide(run_scope));
					yield* Effect.forkScoped(inactivity).pipe(Scope.provide(run_scope));
					yield* Scope.addFinalizer(
						run_scope,
						events.Finish("closed").pipe(Effect.ignore),
					);
					const lock = yield* Semaphore.make(1);
					const Send = (
						command: EngineCommand,
					): Effect.Effect<void, EngineCommandFailure> =>
						Semaphore.withPermit(lock)(
							Effect.gen(function* () {
								const intent = JSON.stringify(command);
								const current = yield* Ref.get(state);
								const prior = current.command_intents.get(command.command_id);
								if (prior !== undefined)
									return prior === intent
										? undefined
										: yield* Effect.fail(
												new EngineCommandIdConflictError({
													artisan_run_id: input.artisan_run_id,
													command_id: command.command_id,
												}),
											);
								if ((yield* events.IsClosed) && command._tag !== "close")
									return yield* Effect.fail(
										new EngineRunClosedError({
											artisan_run_id: input.artisan_run_id,
											command_id: command.command_id,
										}),
									);
								yield* Ref.update(state, (value) => ({
									...value,
									command_intents: new Map(value.command_intents).set(
										command.command_id,
										intent,
									),
								}));
								if (command._tag === "cancel" || command._tag === "close")
									return yield* events.Finish(
										command._tag === "cancel" ? "cancelled" : "closed",
									);
								if (command._tag === "steer")
									return yield* handle.Write(
										new TextEncoder().encode(
											`${JSON.stringify(ToUserMessage(session_id, command.text, command.content ?? undefined))}\n`,
										),
									);
								if (command._tag === "respond_approval") {
									const pending = current.pending.get(command.approval_id);
									if (pending === undefined)
										return yield* Effect.fail(
											new EngineCommandTargetError({
												artisan_run_id: input.artisan_run_id,
												command_id: command.command_id,
												target: "approval",
												target_id: command.approval_id,
											}),
										);
									yield* handle.Write(
										new TextEncoder().encode(
											`${JSON.stringify(ApprovalResponse(command.approval_id, command.approved))}\n`,
										),
									);
									yield* Ref.update(state, (value) => {
										const pending_approvals = new Map(value.pending);
										pending_approvals.delete(command.approval_id);
										return { ...value, pending: pending_approvals };
									});
									return yield* events.Emit({
										_tag: "approval",
										approval_id: command.approval_id,
										approved: command.approved,
										artisan_run_id: input.artisan_run_id,
										description: pending.description,
										observation_id: `${input.artisan_run_id}:claude:${command.approval_id}:resolved`,
										raw: {
											engine_id: "claude",
											frame: ApprovalResponse(
												command.approval_id,
												command.approved,
											),
											protocol_version: claude_protocol_version,
											transport: claude_transport,
										},
										request: pending.request,
										sequence: 0,
										state: "resolved",
									});
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
						Closed: events.Closed,
						Events: events.Events,
						native_thread_id: session_id,
						resume_token: { native_thread_id: session_id },
						Send,
					} satisfies EngineRun;
				});
			const Usage: Required<Engine>["Usage"] = MakeClaudeUsage({
				executable: options.executable,
				executable_args: options.executable_args,
				factory,
			});
			return {
				CheckNativeContinuation: (request) =>
					Probe({}).pipe(
						Effect.map((probe) =>
							probe.version === claude_native_continuation_version &&
							request.target_model !== undefined
								? { state: "compatible" as const }
								: {
										state: "unsupported" as const,
										reason: `Claude CLI native continuation requires version ${claude_native_continuation_version} and a target model`,
									},
						),
					),
				Descriptor: ClaudeEngineDescriptor,
				Open,
				Probe,
				Usage,
			};
		}),
	);
};

/** Explicit transport aliases retained for focused adapter tests. */
export {
	ClaudeEngine as ClaudeCliEngine,
	make_claude_engine_layer as make_claude_cli_engine_layer,
};
