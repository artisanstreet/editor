import { isAbsolute, join } from "node:path";
import { NodeFileSystem } from "@effect/platform-node-shared";

import {
	Context,
	Effect,
	Encoding,
	Exit,
	FileSystem,
	Layer,
	Ref,
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	type Engine,
	type EngineApprovalObservation,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineDescriptor,
	type EngineFailure,
	type EngineObservation,
	type EngineOpenInput,
	type EngineUserInputPart,
	type EngineRun,
	EngineCommandIdConflictError,
	EngineCommandTargetError,
	EngineConfigurationError,
	EngineProcessError,
	EngineProtocolError,
	EngineRunClosedError,
	EngineUnavailableError,
} from "../engine";
import { WatchEngineInactivity } from "../process/inactivity-deadline";
import { normalise_codex_notification } from "./normalizer";
import { make_codex_exec_engine } from "./exec-engine";
import {
	open_codex_app_server_session,
	type CodexAppServerDiagnostic,
	type CodexAppServerSessionFailure,
} from "./app-server-session";
import { CodexProcessFactory, type CodexProcessSpawnInput } from "./process";
import { CodexTransportMetadata } from "./protocol";
import { MakeCodexUsage } from "./usage";
import {
	CodexRuntimeEnvironment,
	CodexRuntimeEnvironmentLive,
	make_codex_process_environment,
	ResolveCodexExecutable,
	resolve_codex_executable,
} from "./executable";
import { CodexAccountReadSchema, codex_selection_failure_reason, MakeCodexProbe } from "./probe";
import { MakeCodexAppServerEventBuffer } from "./internal/app-server-event-buffer";
import { MakeCodexAppServerThreadOptions } from "./internal/permissions";

export {
	CodexRuntimeEnvironment,
	CodexRuntimeEnvironmentLive,
	make_codex_process_environment,
	resolve_codex_executable,
} from "./executable";

/** Identifies the Codex adapter and its currently available capabilities. @since 0.3.0 */
export const CodexEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "experimental",
			reason: "Command and file approvals are supported; permission-profile approvals are not yet canonicalized.",
		},
		auth: { state: "supported" },
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: { state: "supported" },
		global_guidance: { state: "supported" },
		model_selection: { state: "supported" },
		native_continuation: {
			state: "experimental",
			reason: "Codex 0.145.0 validates the requested model before explicit native resume.",
		},
		native_tools: {
			state: "experimental",
			reason: "Native tools are surfaced through normalized activity where known.",
		},
		probe: { state: "supported" },
		question: {
			state: "experimental",
			reason: "Codex request_user_input remains an experimental provider capability.",
		},
		raw_frames: { state: "supported" },
		resume: { state: "supported" },
		start: { state: "supported" },
		steer: { state: "supported" },
		subagents: {
			state: "experimental",
			reason: "Subagent and collaboration activity remains provider-native.",
		},
	},
	display_name: "Codex",
	id: "codex",
	transport: CodexTransportMetadata.transport,
};

/** Configures the Codex executable, transport buffers, and non-billable probe deadlines. @since 0.3.0 */
export interface CodexEngineOptions {
	/** Silence during an active turn that settles it as stalled. */
	readonly app_server_inactivity_ms?: number;
	readonly app_server_max_frame_bytes?: number;
	readonly executable_args?: ReadonlyArray<string>;
	readonly event_capacity?: number;
	readonly executable?: string;
	readonly exec_max_frame_bytes?: number;
	readonly exec_max_stderr_bytes?: number;
	readonly exec_max_stdout_bytes?: number;
	/** Silence that settles an exec run as stalled; every observation re-arms it. */
	readonly exec_inactivity_ms?: number;
	readonly initialize_timeout_ms?: number;
	readonly request_timeout_ms?: number;
	readonly transport_selection?: "app_server_only" | "prefer_app_server_with_exec_fallback";
	readonly version_timeout_ms?: number;
}

/** Provides a dependency-free Codex engine assembled by its Layer. @since 0.3.0 */
export class CodexEngine extends Context.Service<CodexEngine, Engine>()("Artisan/CodexEngine") {}

interface CodexRunState {
	readonly active_turn_id: string | undefined;
	readonly approvals: ReadonlyMap<string, PendingApproval>;
	readonly command_intents: ReadonlyMap<string, string>;
	readonly questions: ReadonlyMap<string, PendingQuestion>;
}

interface PendingApproval {
	readonly description: string;
	readonly native_request_id: string | number;
	readonly request: EngineApprovalObservation["request"];
}

interface PendingQuestion {
	readonly native_request_id: string | number;
	readonly text: string;
}

const ThreadResponseSchema = Schema.Struct({
	thread: Schema.Struct({
		id: Schema.String,
		turns: Schema.optional(
			Schema.Array(
				Schema.Struct({
					id: Schema.String,
					status: Schema.Literals(["completed", "failed", "inProgress", "interrupted"]),
				}),
			),
		),
	}),
});
const TurnResponseSchema = Schema.Struct({ turn: Schema.Struct({ id: Schema.String }) });

function ValidateEventCapacity(event_capacity: number) {
	return Number.isSafeInteger(event_capacity) && event_capacity > 0
		? Effect.void
		: Effect.fail(
				new EngineConfigurationError({
					engine_id: "codex",
					option: "event_capacity",
					value: event_capacity,
				}),
			);
}

function command_intent(command: EngineCommand) {
	switch (command._tag) {
		case "steer":
			return JSON.stringify([command._tag, command.text, command.content]);
		case "respond_approval":
			return JSON.stringify([command._tag, command.approval_id, command.approved]);
		case "respond_question":
			return JSON.stringify([
				command._tag,
				Object.entries(command.answers).sort(([left], [right]) =>
					left.localeCompare(right),
				),
			]);
		case "cancel":
		case "close":
			return JSON.stringify([command._tag]);
	}
}

function terminal_for_turn(state: "cancelled" | "completed" | "failed" | "started" | "waiting") {
	return state === "completed"
		? "completed"
		: state === "cancelled"
			? "cancelled"
			: state === "failed"
				? "failed"
				: undefined;
}

function map_session_failure(
	error: CodexAppServerSessionFailure,
): EngineProcessError | EngineProtocolError {
	if (error instanceof EngineProcessError) {
		return error;
	}

	const message = (() => {
		switch (error._tag) {
			case "CodexAppServerClosedError":
				return `Codex app-server closed: ${error.reason}`;
			case "CodexAppServerConfigurationError":
				return `Invalid Codex app-server option ${error.option}: ${error.value}`;
			case "CodexAppServerNotificationOverflowError":
				return `Codex notification ingress exceeded capacity ${error.capacity}`;
			case "CodexAppServerProtocolError":
			case "CodexAppServerSerializationError":
				return error.message;
			case "CodexAppServerRequestTimeoutError":
				return `Codex ${error.method} request ${error.id} timed out after ${error.timeout_ms}ms`;
			case "CodexAppServerResponseError":
				return `Codex ${error.method} request ${error.id} failed (${error.error.code}): ${error.error.message}`;
		}
	})();

	return new EngineProtocolError({ engine_id: "codex", message });
}

function MapSessionFailure<A, R>(
	effect: Effect.Effect<A, CodexAppServerSessionFailure, R>,
): Effect.Effect<A, EngineProcessError | EngineProtocolError, R> {
	return effect.pipe(Effect.mapError(map_session_failure));
}

function map_diagnostic(
	input: CodexAppServerDiagnostic,
	artisan_run_id: string,
): EngineObservation {
	return {
		_tag: "process_diagnostic",
		artisan_run_id,
		level: input.level,
		message: input.message,
		observation_id: `${artisan_run_id}:diagnostic:${input.frame_sequence ?? "process"}`,
		raw: {
			engine_id: "codex",
			frame: input.message,
			...(input.frame_sequence === undefined ? {} : { frame_sequence: input.frame_sequence }),
			protocol_version: CodexTransportMetadata.protocol_version,
			...(input.raw_frame_base64 === undefined
				? {}
				: { raw_frame_base64: input.raw_frame_base64 }),
			transport: CodexTransportMetadata.transport,
		},
		sequence: 0,
	};
}

function make_turn_input(text: string, content: ReadonlyArray<EngineUserInputPart> | undefined) {
	const parts = content ?? [{ text, type: "text" }];

	return parts.map((part) =>
		part.type === "text"
			? { text: part.text, text_elements: [], type: "text" }
			: {
					type: "image",
					url: `data:${part.media_type};base64,${Encoding.encodeBase64(part.bytes)}`,
				},
	);
}

function make_codex_app_server_engine(
	factory: typeof CodexProcessFactory.Service,
	options: CodexEngineOptions,
): Engine {
	const app_server_max_frame_bytes = options.app_server_max_frame_bytes ?? 8 * 1_024 * 1_024;
	const executable = options.executable ?? resolve_codex_executable();
	const event_capacity = options.event_capacity ?? 256;
	const executable_args = options.executable_args ?? [];
	/**
	 * Never shorter than the total budget this replaced, so no run that used to
	 * complete can start failing for silence it was previously allowed.
	 */
	const app_server_inactivity_ms = options.app_server_inactivity_ms ?? 30 * 60 * 1_000;
	const request_timeout_ms = options.request_timeout_ms ?? 10_000;
	const initialize_timeout_ms = options.initialize_timeout_ms ?? request_timeout_ms;
	const version_timeout_ms = options.version_timeout_ms ?? 5_000;
	const spawn = {
		args: [...executable_args, "app-server", "--stdio"],
		command: executable,
	};
	const OpenSession = () =>
		open_codex_app_server_session({
			max_frame_bytes: app_server_max_frame_bytes,
			request_timeout_ms,
			spawn,
		}).pipe(Effect.provideService(CodexProcessFactory, factory), MapSessionFailure);
	const { CheckNativeContinuation, Probe, ReadTransportVersion } = MakeCodexProbe({
		descriptor: CodexEngineDescriptor,
		executable,
		executable_args,
		factory,
		initialize_timeout_ms,
		OpenSession,
		version_timeout_ms,
	});
	const Open = (input: EngineOpenInput): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> =>
		Effect.gen(function* () {
			yield* ValidateEventCapacity(event_capacity);
			const thread_options = yield* MakeCodexAppServerThreadOptions(input);

			yield* ReadTransportVersion;

			const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
				Scope.close(scope, Exit.succeed(undefined)),
			);
			const session = yield* OpenSession().pipe(Scope.provide(run_scope));
			const command_lock = yield* Semaphore.make(1);
			const state = yield* Ref.make<CodexRunState>({
				active_turn_id: undefined,
				approvals: new Map(),
				command_intents: new Map(),
				questions: new Map(),
			});
			const event_buffer = yield* MakeCodexAppServerEventBuffer({
				artisan_run_id: input.artisan_run_id,
				capacity: event_capacity,
				CloseSession: session.Close.pipe(Effect.ignore),
			});
			const Finish = event_buffer.Finish;
			const Emit = event_buffer.Emit;
			const RememberObservation = (observation: EngineObservation) =>
				Ref.update(state, (current) => {
					if (
						observation._tag === "approval" &&
						observation.state === "requested" &&
						observation.raw.native_id !== undefined
					) {
						return {
							...current,
							approvals: new Map(current.approvals).set(observation.approval_id, {
								description: observation.description,
								native_request_id: observation.raw.native_id,
								request: observation.request,
							}),
						};
					}

					if (
						observation._tag === "question" &&
						observation.state === "requested" &&
						observation.raw.native_id !== undefined
					) {
						return {
							...current,
							questions: new Map(current.questions).set(observation.question_id, {
								native_request_id: observation.raw.native_id,
								text: observation.text,
							}),
						};
					}

					if (observation._tag === "turn_state") {
						return {
							...current,
							active_turn_id:
								observation.state === "started" || observation.state === "waiting"
									? observation.turn_id
									: current.active_turn_id === observation.turn_id
										? undefined
										: current.active_turn_id,
						};
					}

					return current;
				});
			const ProcessObservation = (observation: EngineObservation) =>
				Effect.gen(function* () {
					const active_turn_id = (yield* Ref.get(state)).active_turn_id;
					const terminal =
						observation._tag === "turn_state" && observation.turn_id === active_turn_id
							? terminal_for_turn(observation.state)
							: undefined;

					yield* RememberObservation(observation);
					yield* Emit(observation);

					if (terminal) {
						yield* Finish(terminal);
					}
				});
			const PumpNotifications = session.Notifications.pipe(
				Stream.runForEach((notification) =>
					Semaphore.withPermit(command_lock)(
						normalise_codex_notification({
							artisan_run_id: input.artisan_run_id,
							frame_sequence: notification.frame_sequence,
							...(notification.id === undefined ? {} : { id: notification.id }),
							method: notification.method,
							payload: notification.params,
							protocol_version: CodexTransportMetadata.protocol_version,
							raw_frame_base64: notification.raw_frame_base64,
							transport: CodexTransportMetadata.transport,
						}).pipe(
							Effect.flatMap((observations) =>
								Effect.forEach(observations, ProcessObservation).pipe(
									Effect.andThen(
										notification.method === "thread/closed"
											? Finish("closed")
											: Effect.void,
									),
								),
							),
						),
					),
				),
			).pipe(
				Effect.catch(() => Finish("failed")),
				Effect.ensuring(Finish("failed")),
			);
			const PumpDiagnostics = session.Diagnostics.pipe(
				Stream.runForEach((diagnostic) =>
					Emit(map_diagnostic(diagnostic, input.artisan_run_id)).pipe(Effect.ignore),
				),
			).pipe(Effect.catch(() => Finish("failed")));

			yield* MapSessionFailure(
				session.Handshake({ client_name: "artisan-editor", client_version: "0.3.0" }),
			);
			const account_response = yield* MapSessionFailure(session.Request("account/read", {}));
			const account = yield* Schema.decodeUnknownEffect(CodexAccountReadSchema)(
				account_response.result,
			).pipe(
				Effect.mapError(
					() =>
						new EngineProtocolError({
							engine_id: "codex",
							message: "Codex account/read returned an invalid result",
						}),
				),
			);

			if (!account.account) {
				yield* Finish("failed");

				return yield* Effect.fail(
					new EngineUnavailableError({
						engine_id: "codex",
						message: "Codex app-server is not authenticated with ChatGPT or an API key",
					}),
				);
			}

			const thread_response = yield* MapSessionFailure(
				session.Request(
					input._tag === "start" ? "thread/start" : "thread/resume",
					input._tag === "start"
						? thread_options
						: {
								...thread_options,
								threadId: input.resume_token.native_thread_id,
							},
				),
			);
			const thread = yield* Schema.decodeUnknownEffect(ThreadResponseSchema, {
				onExcessProperty: "preserve",
			})(thread_response.result).pipe(
				Effect.mapError(
					() =>
						new EngineProtocolError({
							engine_id: "codex",
							message: "Codex thread request returned an invalid result",
						}),
				),
			);
			const resumed_active_turn = [...(thread.thread.turns ?? [])]
				.reverse()
				.find((turn) => turn.status === "inProgress");

			if (resumed_active_turn) {
				yield* Ref.update(state, (current) => ({
					...current,
					active_turn_id: resumed_active_turn.id,
				}));
			}

			const initial_text = input._tag === "start" ? input.initial_text : input.next_text;
			const initial_content =
				input._tag === "start" ? input.initial_content : input.next_content;

			if (initial_text !== undefined || (initial_content?.length ?? 0) > 0) {
				const turn_response = yield* MapSessionFailure(
					session.Request("turn/start", {
						input: make_turn_input(initial_text ?? "", initial_content),
						...(input.provider_options?.["codex.service_tier"] === "fast"
							? { serviceTier: "fast" }
							: {}),
						threadId: thread.thread.id,
					}),
				);
				const turn = yield* Schema.decodeUnknownEffect(TurnResponseSchema, {
					onExcessProperty: "preserve",
				})(turn_response.result).pipe(
					Effect.mapError(
						() =>
							new EngineProtocolError({
								engine_id: "codex",
								message: "Codex turn/start returned an invalid result",
							}),
					),
				);

				yield* Ref.update(state, (current) => ({
					...current,
					active_turn_id: turn.turn.id,
				}));
			}

			/**
			 * The app-server session outlives any single turn, so only a turn
			 * already in flight owes output. An idle session between turns is
			 * silent by design and must never be settled for it.
			 */
			const WatchInactivity = WatchEngineInactivity({
				Activity: event_buffer.Activity,
				Closed: event_buffer.Closed,
				Expecting: Ref.get(state).pipe(
					Effect.map((current) => current.active_turn_id !== undefined),
				),
				inactivity_ms: app_server_inactivity_ms,
				OnStall: Emit(
					map_diagnostic(
						{
							level: "error",
							message: `Codex produced no output for ${app_server_inactivity_ms}ms and the turn is treated as stalled`,
							source: "process",
						},
						input.artisan_run_id,
					),
				).pipe(Effect.andThen(Finish("failed"))),
			});

			yield* Effect.forkScoped(PumpNotifications).pipe(Scope.provide(run_scope));
			yield* Effect.forkScoped(PumpDiagnostics).pipe(Scope.provide(run_scope));
			yield* Effect.forkScoped(WatchInactivity).pipe(Scope.provide(run_scope));
			yield* Scope.addFinalizer(run_scope, Finish("closed"));

			const RememberCommand = (command_id: string, intent: string) =>
				Ref.update(state, (current) => ({
					...current,
					command_intents: new Map(current.command_intents).set(command_id, intent),
				}));

			const Send = (command: EngineCommand): Effect.Effect<void, EngineCommandFailure> =>
				Semaphore.withPermit(command_lock)(
					Effect.gen(function* () {
						const current = yield* Ref.get(state);
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

						if (command._tag === "close") {
							yield* RememberCommand(command.command_id, intent);
							yield* Scope.close(run_scope, Exit.succeed(undefined));

							return;
						}

						if (command._tag === "steer") {
							if (!current.active_turn_id) {
								return yield* Effect.fail(
									new EngineProtocolError({
										engine_id: "codex",
										message: "Codex has no active native turn to steer",
									}),
								);
							}

							yield* RememberCommand(command.command_id, intent);
							yield* MapSessionFailure(
								session.Request("turn/steer", {
									expectedTurnId: current.active_turn_id,
									input: make_turn_input(command.text, command.content),
									threadId: thread.thread.id,
								}),
							);
						}

						if (command._tag === "cancel") {
							yield* RememberCommand(command.command_id, intent);

							if (current.active_turn_id) {
								yield* MapSessionFailure(
									session.Request("turn/interrupt", {
										threadId: thread.thread.id,
										turnId: current.active_turn_id,
									}),
								);
							}

							yield* Finish("cancelled");
						}

						if (command._tag === "respond_approval") {
							const pending = current.approvals.get(command.approval_id);

							if (pending === undefined) {
								return yield* Effect.fail(
									new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "approval",
										target_id: command.approval_id,
									}),
								);
							}

							yield* RememberCommand(command.command_id, intent);
							yield* MapSessionFailure(
								session.Respond(pending.native_request_id, {
									decision: command.approved ? "accept" : "decline",
								}),
							);
							yield* Ref.update(state, (next) => {
								const approvals = new Map(next.approvals);

								approvals.delete(command.approval_id);

								return { ...next, approvals };
							});
							yield* Emit({
								_tag: "approval",
								approval_id: command.approval_id,
								approved: command.approved,
								artisan_run_id: input.artisan_run_id,
								description: pending.description,
								observation_id: `${input.artisan_run_id}:command:${command.command_id}:approval:${command.approval_id}`,
								raw: {
									engine_id: "codex",
									frame: {
										command: command._tag,
										command_id: command.command_id,
									},
									native_id: pending.native_request_id,
									protocol_version: CodexTransportMetadata.protocol_version,
									transport: CodexTransportMetadata.transport,
								},
								request: pending.request,
								sequence: 0,
								state: "resolved",
							});
						}

						if (command._tag === "respond_question") {
							const pending_questions = Object.entries(command.answers).map(
								([question_id, answers]) => ({
									answers,
									pending: current.questions.get(question_id),
									question_id,
								}),
							);
							const request_ids = new Set(
								pending_questions.map(
									(question) => question.pending?.native_request_id,
								),
							);

							if (request_ids.size !== 1 || request_ids.has(undefined)) {
								const target_id =
									Object.keys(command.answers).find(
										(question_id) => !current.questions.has(question_id),
									) ?? "multiple-request-groups";

								return yield* Effect.fail(
									new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "question",
										target_id,
									}),
								);
							}

							const request_id = [...request_ids].find(
								(candidate): candidate is string | number =>
									candidate !== undefined,
							);
							if (request_id === undefined) {
								return yield* Effect.fail(
									new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "question",
										target_id: "missing-request-group",
									}),
								);
							}
							const request_question_ids = [...current.questions.entries()]
								.filter(([, question]) => question.native_request_id === request_id)
								.map(([question_id]) => question_id)
								.sort();
							const answered_question_ids = Object.keys(command.answers).sort();
							const has_complete_request_group =
								request_question_ids.length === answered_question_ids.length &&
								request_question_ids.every(
									(question_id, index) =>
										question_id === answered_question_ids[index],
								);

							if (!has_complete_request_group) {
								return yield* Effect.fail(
									new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "question",
										target_id: "incomplete-request-group",
									}),
								);
							}

							yield* RememberCommand(command.command_id, intent);
							yield* MapSessionFailure(
								session.Respond(request_id, {
									answers: Object.fromEntries(
										Object.entries(command.answers).map(
											([question_id, answers]) => [question_id, { answers }],
										),
									),
								}),
							);
							yield* Ref.update(state, (next) => {
								const questions = new Map(next.questions);

								for (const question_id of Object.keys(command.answers)) {
									questions.delete(question_id);
								}

								return { ...next, questions };
							});

							for (const question of pending_questions) {
								if (question.pending === undefined) {
									return yield* Effect.fail(
										new EngineCommandTargetError({
											artisan_run_id: input.artisan_run_id,
											command_id: command.command_id,
											target: "question",
											target_id: question.question_id,
										}),
									);
								}

								yield* Emit({
									_tag: "question",
									answers: question.answers,
									artisan_run_id: input.artisan_run_id,
									observation_id: `${input.artisan_run_id}:command:${command.command_id}:question:${question.question_id}`,
									question_id: question.question_id,
									raw: {
										engine_id: "codex",
										frame: {
											command: command._tag,
											command_id: command.command_id,
										},
										native_id: question.pending.native_request_id,
										protocol_version: CodexTransportMetadata.protocol_version,
										transport: CodexTransportMetadata.transport,
									},
									sequence: 0,
									state: "resolved",
									text: question.pending.text,
								});
							}
						}
					}).pipe(Effect.uninterruptible),
				);

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: event_buffer.Closed,
				Events: event_buffer.Events,
				native_thread_id: thread.thread.id,
				resume_token: {
					native_thread_id: thread.thread.id,
					...(input._tag === "resume" && input.resume_token.opaque_checkpoint
						? { opaque_checkpoint: input.resume_token.opaque_checkpoint }
						: {}),
				},
				Send,
			};
		});

	return {
		CheckNativeContinuation,
		Descriptor: CodexEngineDescriptor,
		Open,
		Probe,
	};
}

/**
 * Builds the Codex engine Layer and captures its process factory at composition.
 *
 * @since 0.3.0
 * @param options - Executable override and bounded transport configuration.
 * @returns A Layer whose public engine has no process dependency in its Effects.
 */
export function make_codex_engine_layer(
	options: CodexEngineOptions = {},
): Layer.Layer<CodexEngine, never, CodexProcessFactory> {
	return Layer.effect(
		CodexEngine,
		Effect.gen(function* () {
			const base_factory = yield* CodexProcessFactory;
			const file_system = yield* FileSystem.FileSystem;
			const runtime_environment = yield* CodexRuntimeEnvironment;
			const executable = options.executable ?? (yield* ResolveCodexExecutable);
			const factory = {
				Spawn: (input: CodexProcessSpawnInput) =>
					Effect.gen(function* () {
						if (
							isAbsolute(input.command) &&
							!(yield* file_system
								.exists(input.command)
								.pipe(Effect.orElseSucceed(() => false)))
						) {
							return yield* Effect.fail(
								new EngineProcessError({
									cause: new Error(
										`Engine executable does not exist: ${input.command}`,
									),
									operation: "spawn",
								}),
							);
						}

						return yield* base_factory.Spawn({
							...input,
							env: make_codex_process_environment(
								input.env,
								runtime_environment.inherited_environment,
								runtime_environment.user_profile,
							),
						});
					}),
			};
			const codex_home =
				make_codex_process_environment(
					{},
					runtime_environment.inherited_environment,
					runtime_environment.user_profile,
				).CODEX_HOME ?? join(runtime_environment.user_profile, ".codex");
			const Usage = MakeCodexUsage({
				codex_home,
				executable,
				executable_args: options.executable_args ?? [],
				factory,
				file_system,
				request_timeout_ms: options.request_timeout_ms ?? 10_000,
			});
			const app_server_engine: Engine = {
				...make_codex_app_server_engine(factory, {
					...options,
					executable,
				}),
				Usage,
			};

			if (options.transport_selection === "app_server_only") {
				return app_server_engine;
			}

			const probe = yield* app_server_engine
				.Probe({ client_name: "artisan-transport-selection", client_version: "0.3.0" })
				.pipe(Effect.exit);

			if (Exit.isSuccess(probe) && probe.value.ready) {
				return app_server_engine;
			}

			return {
				...make_codex_exec_engine({
					event_capacity: options.event_capacity ?? 256,
					executable,
					executable_args: options.executable_args ?? [],
					fallback_reason: codex_selection_failure_reason(probe),
					file_system,
					factory,
					inactivity_ms: options.exec_inactivity_ms ?? 30 * 60 * 1_000,
					max_frame_bytes: options.exec_max_frame_bytes ?? 256 * 1_024,
					max_stderr_bytes: options.exec_max_stderr_bytes ?? 1_024 * 1_024,
					max_stdout_bytes: options.exec_max_stdout_bytes ?? 8 * 1_024 * 1_024,
					version_timeout_ms: options.version_timeout_ms ?? 5_000,
				}),
				Usage,
			};
		}),
	).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(CodexRuntimeEnvironmentLive),
	);
}
