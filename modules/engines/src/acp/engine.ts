import { Buffer } from "node:buffer";

import * as Acp from "@agentclientprotocol/sdk";
import type {
	ContentBlock,
	CreateElicitationRequest,
	CreateElicitationResponse,
	InitializeResponse,
	PromptResponse,
	RequestPermissionRequest,
	RequestPermissionResponse,
} from "@agentclientprotocol/sdk";
import { Deferred, Effect, Exit, Ref, Scope, Semaphore } from "effect";

import {
	type Engine,
	type EngineApprovalRequest,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineDescriptor,
	type EngineFailure,
	type EngineOpenInput,
	type EngineProbe,
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
	ValidateEngineGlobalGuidance,
	ValidateEngineProductInstructions,
} from "../engine";
import { MakeEngineEventBuffer } from "../process/event-buffer";
import { engine_exit_is_interruption } from "../process/exit-classification";
import { EngineProcessFactory, type EngineProcessHandle } from "../process/process";
import {
	with_engine_spawn_override,
	type ResolveEngineSpawnOverride,
} from "../process/spawn-override";
import { CompleteAcpMessages, EmptyAcpNormalizationState, NormalizeAcpUpdate } from "./normalizer";

export const acp_protocol_version = "acp-v1";

export interface AcpEngineDefinition {
	readonly AcpArgs: (input: EngineOpenInput) => ReadonlyArray<string>;
	readonly AuthMethod: (
		initialize: InitializeResponse,
		environment: NodeJS.ProcessEnv,
	) => string | undefined;
	readonly auth_probe_args: ReadonlyArray<string>;
	readonly Authenticated: (output: string) => boolean;
	readonly descriptor: EngineDescriptor;
	readonly executable: string;
	readonly executable_args?: ReadonlyArray<string>;
	readonly image_input: "embedded" | "image" | "unsupported";
	/** Classifies a bounded provider diagnostic emitted while ACP is becoming ready. */
	readonly ClassifyStartupFailure?: (input: {
		readonly cause: unknown;
		readonly operation: string;
		readonly stderr: string;
	}) => EngineFailure | undefined;
	/** Optional provider-owned account-usage surface. */
	readonly Usage?: Engine["Usage"];
	readonly version_args?: ReadonlyArray<string>;
	readonly Version: (output: string) => string | undefined;
}

export interface AcpEngineOptions {
	readonly ResolveSpawnOverride?: ResolveEngineSpawnOverride;
	readonly definition: AcpEngineDefinition;
	readonly max_probe_output_bytes?: number;
}

interface PendingApproval {
	readonly description: string;
	readonly request: EngineApprovalRequest;
	readonly Resolve: (approved: boolean) => void;
}

interface PendingQuestion {
	readonly question_ids: ReadonlyArray<string>;
	readonly Resolve: (answers: Readonly<Record<string, ReadonlyArray<string>>>) => void;
}

interface CursorQuestion {
	readonly allowMultiple?: boolean;
	readonly id: string;
	readonly options: ReadonlyArray<{ readonly id: string; readonly label: string }>;
	readonly prompt: string;
}

interface CursorQuestionRequest {
	readonly questions: ReadonlyArray<CursorQuestion>;
	readonly title?: string;
	readonly toolCallId: string;
}

interface CursorPlanRequest {
	readonly name?: string;
	readonly overview?: string;
	readonly plan: string;
	readonly todos: ReadonlyArray<{
		readonly content: string;
		readonly id: string;
		readonly status: "cancelled" | "completed" | "in_progress" | "pending";
	}>;
	readonly toolCallId: string;
}

const as_record = (value: unknown): Readonly<Record<string, unknown>> => {
	if (typeof value !== "object" || value === null || Array.isArray(value))
		throw new Error("Expected an object");
	return value as Readonly<Record<string, unknown>>;
};

const parse_cursor_question = (value: unknown): CursorQuestionRequest => {
	const item = as_record(value);
	if (typeof item.toolCallId !== "string" || !Array.isArray(item.questions))
		throw new Error("Invalid Cursor question request");
	return {
		questions: item.questions.map((question) => {
			const candidate = as_record(question);
			if (
				typeof candidate.id !== "string" ||
				typeof candidate.prompt !== "string" ||
				!Array.isArray(candidate.options)
			)
				throw new Error("Invalid Cursor question");
			return {
				allowMultiple: candidate.allowMultiple === true,
				id: candidate.id,
				options: candidate.options.map((option) => {
					const choice = as_record(option);
					if (typeof choice.id !== "string" || typeof choice.label !== "string")
						throw new Error("Invalid Cursor question option");
					return { id: choice.id, label: choice.label };
				}),
				prompt: candidate.prompt,
			};
		}),
		...(typeof item.title === "string" ? { title: item.title } : {}),
		toolCallId: item.toolCallId,
	};
};

const parse_cursor_plan = (value: unknown): CursorPlanRequest => {
	const item = as_record(value);
	if (
		typeof item.toolCallId !== "string" ||
		typeof item.plan !== "string" ||
		!Array.isArray(item.todos)
	)
		throw new Error("Invalid Cursor plan request");
	return {
		...(typeof item.name === "string" ? { name: item.name } : {}),
		...(typeof item.overview === "string" ? { overview: item.overview } : {}),
		plan: item.plan,
		todos: item.todos.flatMap((todo) => {
			const entry = as_record(todo);
			return typeof entry.id === "string" &&
				typeof entry.content === "string" &&
				(entry.status === "cancelled" ||
					entry.status === "completed" ||
					entry.status === "in_progress" ||
					entry.status === "pending")
				? [
						{
							content: entry.content,
							id: entry.id,
							status: entry.status,
						},
					]
				: [];
		}),
		toolCallId: item.toolCallId,
	};
};

const readable_stream = (source: AsyncIterable<Uint8Array>) => {
	const iterator = source[Symbol.asyncIterator]();
	return new ReadableStream<Uint8Array>({
		async cancel() {
			await iterator.return?.();
		},
		async pull(controller) {
			try {
				const next = await iterator.next();
				if (next.done) controller.close();
				else controller.enqueue(next.value);
			} catch (cause) {
				controller.error(cause);
			}
		},
	});
};

const acp_stream = (handle: EngineProcessHandle) =>
	Acp.ndJsonStream(
		new WritableStream<Uint8Array>({
			close: () => Effect.runPromise(handle.EndInput),
			write: (chunk) => Effect.runPromise(handle.Write(chunk)),
		}),
		readable_stream(handle.Stdout),
	);

const ReadOutput = (source: AsyncIterable<Uint8Array>, maximum_bytes: number) =>
	Effect.tryPromise({
		try: async () => {
			let size = 0;
			let output = "";
			for await (const chunk of source) {
				size += chunk.byteLength;
				if (size > maximum_bytes) throw new Error("Engine probe output exceeded its bound");
				output += new TextDecoder().decode(chunk);
			}
			return output;
		},
		catch: (cause) => new EngineProcessError({ cause, operation: "read" }),
	});

const RunProbeCommand = (
	factory: typeof EngineProcessFactory.Service,
	definition: AcpEngineDefinition,
	args: ReadonlyArray<string>,
	maximum_bytes: number,
) =>
	Effect.scoped(
		Effect.gen(function* () {
			const handle = yield* factory.Spawn({
				args: [...(definition.executable_args ?? []), ...args],
				command: definition.executable,
				env: process.env,
			});
			const [stdout, stderr, exit] = yield* Effect.all(
				[
					ReadOutput(handle.Stdout, maximum_bytes),
					ReadOutput(handle.Stderr, maximum_bytes),
					handle.Exit,
				],
				{ concurrency: "unbounded" },
			).pipe(Effect.ensuring(handle.Close));
			return { exit, output: `${stdout}\n${stderr}`.trim() };
		}),
	);

const prompt_content = (
	definition: AcpEngineDefinition,
	text: string,
	content: ReadonlyArray<EngineUserInputPart> | undefined,
	product_instructions: string | undefined,
): ReadonlyArray<ContentBlock> => {
	const parts: Array<ContentBlock> = [];
	if (product_instructions !== undefined)
		parts.push({
			type: "text",
			text: `<artisan-product-instructions>\n${product_instructions}\n</artisan-product-instructions>`,
		});
	for (const part of content ?? [{ text, type: "text" as const }]) {
		if (part.type === "text") {
			parts.push({ type: "text", text: part.text });
			continue;
		}
		const data = Buffer.from(part.bytes).toString("base64");
		if (definition.image_input === "image") {
			parts.push({ data, mimeType: part.media_type, type: "image" });
			continue;
		}
		if (definition.image_input === "embedded") {
			parts.push({
				resource: {
					blob: data,
					mimeType: part.media_type,
					uri: `artisan://attachment/${encodeURIComponent(part.id)}/${encodeURIComponent(part.name)}`,
				},
				type: "resource",
			});
			continue;
		}
		throw new EngineConfigurationError({
			engine_id: definition.descriptor.id,
			option: "image_input",
			value: part.media_type,
		});
	}
	return parts.length === 0 ? [{ type: "text", text }] : parts;
};

const approval_request = (request: RequestPermissionRequest): EngineApprovalRequest => {
	const tool = request.toolCall;
	const reason = tool.title ?? undefined;
	if (tool.kind === "execute") {
		const raw =
			typeof tool.rawInput === "object" && tool.rawInput !== null
				? as_record(tool.rawInput)
				: {};
		return {
			...(typeof raw.command === "string" ? { command: raw.command } : {}),
			kind: "command",
			...(reason === undefined ? {} : { reason }),
		};
	}
	if (tool.kind === "edit" || tool.kind === "delete" || tool.kind === "move")
		return { kind: "file_change", ...(reason === undefined ? {} : { reason }) };
	return { kind: "action", ...(reason === undefined ? {} : { reason }) };
};

const resolve_permission = (
	request: RequestPermissionRequest,
	approved: boolean,
): RequestPermissionResponse => {
	const allowed = request.options.find(
		(option) => option.kind === "allow_once" || option.kind === "allow_always",
	);
	const rejected = request.options.find(
		(option) => option.kind === "reject_once" || option.kind === "reject_always",
	);
	const selected = approved ? allowed : rejected;
	return selected === undefined
		? { outcome: { outcome: "cancelled" } }
		: { outcome: { optionId: selected.optionId, outcome: "selected" } };
};

type FormElicitationRequest = Extract<CreateElicitationRequest, { mode: "form" }>;

const question_options = (property: FormElicitationRequest, key: string) => {
	const value = property.requestedSchema.properties?.[key];
	if (value === undefined) return undefined;
	const schema = as_record(value);
	const choices =
		Array.isArray(schema.oneOf) || Array.isArray(schema.enum)
			? (schema.oneOf ?? schema.enum)
			: schema.type === "array" && typeof schema.items === "object" && schema.items !== null
				? (as_record(schema.items).anyOf ?? as_record(schema.items).enum)
				: undefined;
	if (!Array.isArray(choices)) return undefined;
	return choices.flatMap((choice) => {
		if (typeof choice === "string") return [{ label: choice }];
		const option = as_record(choice);
		const label =
			typeof option.title === "string"
				? option.title
				: typeof option.const === "string"
					? option.const
					: undefined;
		return label === undefined
			? []
			: [
					{
						...(typeof option.description === "string"
							? { description: option.description }
							: {}),
						label,
					},
				];
	});
};

const elicitation_content = (
	request: FormElicitationRequest,
	answers: Readonly<Record<string, ReadonlyArray<string>>>,
) => {
	const content: Record<string, string | number | boolean | Array<string>> = {};
	for (const [key, value] of Object.entries(request.requestedSchema.properties ?? {})) {
		const schema = as_record(value);
		const values = answers[key] ?? [];
		switch (schema.type) {
			case "array":
				content[key] = [...values];
				break;
			case "boolean":
				content[key] = values[0]?.toLowerCase() === "true";
				break;
			case "integer":
			case "number":
				content[key] = Number(values[0]);
				break;
			default:
				content[key] = values[0] ?? "";
		}
	}
	return content;
};

/** Builds one process-per-turn ACP adapter shared by Grok Build and Cursor. */
export const MakeAcpEngine = (
	options: AcpEngineOptions,
): Effect.Effect<Engine, never, EngineProcessFactory> =>
	Effect.gen(function* () {
		const definition = options.definition;
		const base_factory = yield* EngineProcessFactory;
		const factory =
			options.ResolveSpawnOverride === undefined
				? base_factory
				: with_engine_spawn_override(base_factory, options.ResolveSpawnOverride);
		const maximum_probe_bytes = options.max_probe_output_bytes ?? 1_048_576;

		const Probe: Engine["Probe"] = () =>
			Effect.gen(function* () {
				const version_result = yield* RunProbeCommand(
					factory,
					definition,
					definition.version_args ?? ["--version"],
					maximum_probe_bytes,
				);
				const version = definition.Version(version_result.output);
				if (version_result.exit.code !== 0 || version === undefined)
					return yield* new EngineUnavailableError({
						engine_id: definition.descriptor.id,
						message: `${definition.descriptor.display_name} did not report a valid version.`,
					});
				const auth_result = yield* RunProbeCommand(
					factory,
					definition,
					definition.auth_probe_args,
					maximum_probe_bytes,
				);
				const authenticated =
					auth_result.exit.code === 0 && definition.Authenticated(auth_result.output);
				return {
					authentication: authenticated
						? { state: "authenticated" as const }
						: {
								reason: `Sign in to ${definition.descriptor.display_name} from Settings.`,
								state: "unauthenticated" as const,
							},
					capabilities: definition.descriptor.capabilities,
					descriptor: definition.descriptor,
					metadata: { protocol: acp_protocol_version },
					ready: authenticated,
					version,
				} satisfies EngineProbe;
			});

		const Open: Engine["Open"] = (input) =>
			Effect.gen(function* () {
				yield* ValidateEngineGlobalGuidance(
					definition.descriptor.id,
					input.global_guidance,
				);
				yield* ValidateEngineProductInstructions(
					definition.descriptor.id,
					input.product_instructions,
				);
				const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
					Scope.close(scope, Exit.void),
				);
				const handle = yield* factory
					.Spawn({
						args: [...(definition.executable_args ?? []), ...definition.AcpArgs(input)],
						command: definition.executable,
						cwd: input.working_directory,
						env: process.env,
						...(input.profile_id === undefined ? {} : { profile_id: input.profile_id }),
					})
					.pipe(Scope.provide(run_scope));
				yield* Scope.addFinalizer(run_scope, handle.Close);
				/**
				 * ACP owns stdout, but provider CLIs put their actionable startup reason on
				 * stderr. Drain it concurrently so the child cannot block and retain only a
				 * small tail for adapter-owned classification.
				 */
				let startup_stderr_tail = "";
				const startup_stderr_closed = yield* Deferred.make<void>();
				const CaptureStartupStderr = Effect.tryPromise({
					try: async () => {
						const decoder = new TextDecoder();
						for await (const chunk of handle.Stderr) {
							startup_stderr_tail = `${startup_stderr_tail}${decoder.decode(chunk, {
								stream: true,
							})}`.slice(-16_384);
						}
						startup_stderr_tail = `${startup_stderr_tail}${decoder.decode()}`.slice(
							-16_384,
						);
					},
					catch: () => undefined,
				}).pipe(
					Effect.ignore,
					Effect.ensuring(Deferred.succeed(startup_stderr_closed, undefined)),
				);
				yield* Effect.forkScoped(CaptureStartupStderr).pipe(Scope.provide(run_scope));

				let native_thread_id =
					input._tag === "resume" ? input.resume_token.native_thread_id : "opening";
				let summary_title: string | undefined;
				let connection: Acp.ClientConnection | undefined;
				const close_resource = Effect.gen(function* () {
					yield* Effect.sync(() => connection?.close());
					yield* handle.Close;
				});
				const events = yield* MakeEngineEventBuffer({
					artisan_run_id: input.artisan_run_id,
					CloseResource: close_resource,
					make_terminal_observation: (state, sequence, error_ref) => ({
						_tag: "run_terminal",
						artisan_run_id: input.artisan_run_id,
						native_thread_id,
						observation_id: `${input.artisan_run_id}:${definition.descriptor.id}:terminal`,
						raw: {
							engine_id: definition.descriptor.id,
							frame: { state },
							protocol_version: acp_protocol_version,
							transport: definition.descriptor.transport,
						},
						sequence,
						state,
						...(error_ref === undefined ? {} : { error_ref }),
						...(summary_title === undefined ? {} : { summary_title }),
					}),
				});
				const approvals = new Map<string, PendingApproval>();
				const questions = new Map<string, PendingQuestion>();
				const command_intents = yield* Ref.make<ReadonlyMap<string, string>>(new Map());
				const send_lock = yield* Semaphore.make(1);
				let frame_sequence = 0;
				let prompt_active = false;
				let normalization = EmptyAcpNormalizationState();
				const turn_id = `${definition.descriptor.id}:${input.artisan_run_id}:turn`;
				const Emit = (observation: Parameters<typeof events.Emit>[0]) =>
					Effect.runPromise(events.Emit(observation));
				const raw_base = (frame: unknown, suffix: string) => ({
					artisan_run_id: input.artisan_run_id,
					native_thread_id,
					observation_id: `${input.artisan_run_id}:${definition.descriptor.id}:acp:${frame_sequence}:${suffix}`,
					raw: {
						engine_id: definition.descriptor.id,
						frame,
						frame_sequence,
						protocol_version: acp_protocol_version,
						transport: definition.descriptor.transport,
					},
					sequence: 0,
				});

				const client = Acp.client({ name: "Artisan Editor" })
					.onNotification(Acp.methods.client.session.update, async ({ params }) => {
						if (params.sessionId !== native_thread_id || !prompt_active) return;
						frame_sequence += 1;
						if (params.update.sessionUpdate === "session_info_update") {
							summary_title = params.update.title ?? summary_title;
							return;
						}
						for (const observation of NormalizeAcpUpdate({
							artisan_run_id: input.artisan_run_id,
							engine_id: definition.descriptor.id,
							frame_sequence,
							native_thread_id,
							protocol_version: acp_protocol_version,
							state: normalization,
							transport: definition.descriptor.transport,
							turn_id,
							update: params.update,
						}))
							await Emit(observation);
					})
					.onRequest(Acp.methods.client.session.requestPermission, async ({ params }) => {
						frame_sequence += 1;
						const request = approval_request(params);
						const description = params.toolCall.title ?? "Approve this tool call?";
						if (input.permission_policy?.approval === "never")
							return resolve_permission(params, input.permission_policy.write_access);
						return new Promise<RequestPermissionResponse>((resolve) => {
							approvals.set(params.toolCall.toolCallId, {
								description,
								request,
								Resolve: (approved) =>
									resolve(resolve_permission(params, approved)),
							});
							void Emit({
								...raw_base(
									params,
									`approval:${params.toolCall.toolCallId}:requested`,
								),
								_tag: "approval",
								approval_id: params.toolCall.toolCallId,
								description,
								request,
								state: "requested",
							});
						});
					})
					.onRequest(Acp.methods.client.elicitation.create, async ({ params }) => {
						frame_sequence += 1;
						if (params.mode !== "form" || !("requestedSchema" in params))
							return { action: "decline" };
						const form = params as FormElicitationRequest;
						const keys = Object.keys(form.requestedSchema.properties ?? {});
						const request_id = `elicitation:${frame_sequence}`;
						return new Promise<CreateElicitationResponse>((resolve) => {
							questions.set(request_id, {
								question_ids: keys,
								Resolve: (answers) =>
									resolve({
										action: "accept",
										content: elicitation_content(form, answers),
									}),
							});
							void Promise.all(
								keys.map((key) => {
									const property_value = form.requestedSchema.properties?.[key];
									const property =
										property_value === undefined
											? undefined
											: as_record(property_value);
									const title =
										typeof property?.title === "string"
											? property.title
											: undefined;
									const description =
										typeof property?.description === "string"
											? property.description
											: undefined;
									const options = question_options(form, key);
									return Emit({
										...raw_base(params, `question:${key}:requested`),
										_tag: "question",
										...(title === undefined ? {} : { header: title }),
										multi_select: property?.type === "array",
										...(options === undefined ? {} : { options }),
										question_id: key,
										state: "requested",
										text: description ?? title ?? form.message,
									});
								}),
							);
						});
					});

				client.onRequest(
					"cursor/ask_question",
					parse_cursor_question,
					async ({ params }) => {
						frame_sequence += 1;
						return new Promise((resolve) => {
							questions.set(params.toolCallId, {
								question_ids: params.questions.map((question) => question.id),
								Resolve: (answers) =>
									resolve({
										outcome: {
											answers: params.questions.map((question) => ({
												questionId: question.id,
												selectedOptionIds: (answers[question.id] ?? []).map(
													(answer) =>
														question.options.find(
															(option) =>
																option.id === answer ||
																option.label === answer,
														)?.id ?? answer,
												),
											})),
											outcome: "answered",
										},
									}),
							});
							void Promise.all(
								params.questions.map((question) =>
									Emit({
										...raw_base(params, `question:${question.id}:requested`),
										_tag: "question",
										...(params.title === undefined
											? {}
											: { header: params.title }),
										multi_select: question.allowMultiple ?? false,
										options: question.options.map((option) => ({
											label: option.label,
										})),
										question_id: question.id,
										state: "requested",
										text: question.prompt,
									}),
								),
							);
						});
					},
				);
				client.onRequest("cursor/create_plan", parse_cursor_plan, async ({ params }) => {
					frame_sequence += 1;
					await Emit({
						...raw_base(params, `plan:${params.toolCallId}`),
						_tag: "plan",
						entries: params.todos
							.filter((todo) => todo.status !== "cancelled")
							.map((todo) => ({
								id: todo.id,
								status: todo.status === "cancelled" ? "pending" : todo.status,
								text: todo.content,
							})),
						turn_id,
					});
					return new Promise((resolve) => {
						approvals.set(params.toolCallId, {
							description: params.overview ?? params.name ?? "Approve this plan?",
							request: { kind: "action", reason: params.plan },
							Resolve: (approved) =>
								resolve({
									outcome: { outcome: approved ? "accepted" : "rejected" },
								}),
						});
						void Emit({
							...raw_base(params, `approval:${params.toolCallId}:requested`),
							_tag: "approval",
							approval_id: params.toolCallId,
							description: params.overview ?? params.name ?? "Approve this plan?",
							request: { kind: "action", reason: params.plan },
							state: "requested",
						});
					});
				});

				connection = client.connect(acp_stream(handle));
				const Request = <Response>(promise: Promise<Response>, operation: string) =>
					Effect.tryPromise({
						try: () => promise,
						catch: (cause) => cause,
					}).pipe(
						Effect.catch((cause) =>
							/** Wait briefly for a closing child's final diagnostic chunk. */
							Effect.race(
								Deferred.await(startup_stderr_closed),
								Effect.sleep("25 millis"),
							).pipe(
								Effect.flatMap(() => {
									const classified = definition.ClassifyStartupFailure?.({
										cause,
										operation,
										stderr: startup_stderr_tail,
									});
									return classified === undefined
										? Effect.fail(
												new EngineProtocolError({
													engine_id: definition.descriptor.id,
													message: `${operation}: ${cause instanceof Error ? cause.message : String(cause)}`,
												}),
											)
										: Effect.fail(classified);
								}),
							),
						),
					);
				const initialize = yield* Request(
					connection.agent.request(Acp.methods.agent.initialize, {
						clientCapabilities: { plan: {}, session: { compaction: {} } },
						clientInfo: { name: "Artisan Editor", version: "0.2.2" },
						protocolVersion: Acp.PROTOCOL_VERSION,
					}),
					"ACP initialize failed",
				);
				if (initialize.protocolVersion !== Acp.PROTOCOL_VERSION)
					return yield* new EngineProtocolError({
						engine_id: definition.descriptor.id,
						message: `ACP protocol ${String(initialize.protocolVersion)} is unsupported.`,
					});
				const auth_method = definition.AuthMethod(initialize, process.env);
				if (auth_method === undefined)
					return yield* new EngineUnavailableError({
						engine_id: definition.descriptor.id,
						message: `Sign in to ${definition.descriptor.display_name} from Settings.`,
					});
				yield* Request(
					connection.agent.request(Acp.methods.agent.authenticate, {
						_meta: { headless: true },
						methodId: auth_method,
					}),
					"ACP authentication failed",
				).pipe(
					Effect.mapError((cause) =>
						cause instanceof EngineUnavailableError
							? cause
							: new EngineUnavailableError({
									engine_id: definition.descriptor.id,
									message: cause.message,
								}),
					),
				);

				if (input._tag === "resume") {
					yield* Request(
						connection.agent.request(Acp.methods.agent.session.load, {
							cwd: input.working_directory,
							mcpServers: [],
							sessionId: native_thread_id,
						}),
						"ACP session load failed",
					);
				} else {
					const created = yield* Request(
						connection.agent.request(Acp.methods.agent.session.new, {
							cwd: input.working_directory,
							mcpServers: [],
						}),
						"ACP session creation failed",
					);
					native_thread_id = created.sessionId;
				}

				yield* events.Emit({
					...raw_base({ type: "session-opened" }, "run:opening"),
					_tag: "run_state",
					state: "opening",
				});

				const RunPrompt = (
					text: string,
					content: ReadonlyArray<EngineUserInputPart> | undefined,
				) =>
					Effect.gen(function* () {
						prompt_active = true;
						normalization = EmptyAcpNormalizationState();
						yield* events.Emit({
							...raw_base({ type: "prompt-start" }, "run:running"),
							_tag: "run_state",
							state: "running",
						});
						yield* events.Emit({
							...raw_base({ type: "prompt-start" }, "turn:started"),
							_tag: "turn_state",
							state: "started",
							turn_id,
						});
						const response = yield* Request<PromptResponse>(
							connection!.agent.request(Acp.methods.agent.session.prompt, {
								prompt: prompt_content(
									definition,
									text,
									content,
									input.product_instructions?.content,
								),
								sessionId: native_thread_id,
							}),
							"ACP prompt failed",
						);
						for (const observation of CompleteAcpMessages({
							artisan_run_id: input.artisan_run_id,
							engine_id: definition.descriptor.id,
							frame_sequence,
							native_thread_id,
							protocol_version: acp_protocol_version,
							state: normalization,
							transport: definition.descriptor.transport,
							turn_id,
						}))
							yield* events.Emit(observation);
						if (response.usage !== null && response.usage !== undefined)
							yield* events.Emit({
								...raw_base(response, "usage:prompt"),
								_tag: "usage",
								basis: "cumulative",
								...(response.usage.cachedReadTokens === null ||
								response.usage.cachedReadTokens === undefined
									? {}
									: { cached_input_tokens: response.usage.cachedReadTokens }),
								input_tokens: response.usage.inputTokens,
								output_tokens: response.usage.outputTokens,
								turn_id,
							});
						const cancelled = response.stopReason === "cancelled";
						yield* events.Emit({
							...raw_base(response, "turn:completed"),
							_tag: "turn_state",
							state: cancelled ? "cancelled" : "completed",
							turn_id,
						});
						prompt_active = false;
						yield* events.Finish(cancelled ? "cancelled" : "completed");
					}).pipe(
						Effect.catch((failure) =>
							events
								.Emit({
									...raw_base(failure, "prompt:failed"),
									_tag: "process_diagnostic",
									level: "error",
									message: failure.message,
								})
								.pipe(Effect.andThen(events.Finish("failed"))),
						),
					);

				const first_text = input._tag === "start" ? input.initial_text : input.next_text;
				const first_content =
					input._tag === "start" ? input.initial_content : input.next_content;
				if (first_text !== undefined || (first_content?.length ?? 0) > 0)
					yield* Effect.forkScoped(RunPrompt(first_text ?? "", first_content)).pipe(
						Scope.provide(run_scope),
					);
				else
					yield* events.Emit({
						...raw_base({ type: "session-waiting" }, "run:waiting"),
						_tag: "run_state",
						state: "waiting",
					});

				const process_exit = handle.Exit.pipe(
					Effect.flatMap((exit) =>
						events.IsClosed.pipe(
							Effect.flatMap((closed) =>
								closed
									? Effect.void
									: events.Finish(
											engine_exit_is_interruption(exit)
												? "interrupted"
												: "failed",
										),
							),
						),
					),
					Effect.catch(() => events.Finish("failed")),
				);
				yield* Effect.forkScoped(process_exit).pipe(Scope.provide(run_scope));
				yield* Scope.addFinalizer(run_scope, events.Finish("closed").pipe(Effect.ignore));

				const Send = (command: EngineCommand): Effect.Effect<void, EngineCommandFailure> =>
					Semaphore.withPermit(send_lock)(
						Effect.gen(function* () {
							const intent = JSON.stringify(command);
							const intents = yield* Ref.get(command_intents);
							const prior = intents.get(command.command_id);
							if (prior !== undefined)
								return prior === intent
									? undefined
									: yield* new EngineCommandIdConflictError({
											artisan_run_id: input.artisan_run_id,
											command_id: command.command_id,
										});
							if ((yield* events.IsClosed) && command._tag !== "close")
								return yield* new EngineRunClosedError({
									artisan_run_id: input.artisan_run_id,
									command_id: command.command_id,
								});
							yield* Ref.update(command_intents, (current) =>
								new Map(current).set(command.command_id, intent),
							);
							if (command._tag === "cancel") {
								yield* Request(
									connection!.agent.notify(Acp.methods.agent.session.cancel, {
										sessionId: native_thread_id,
									}),
									"ACP cancel failed",
								).pipe(Effect.ignore);
								return yield* events.Finish("cancelled");
							}
							if (command._tag === "close") return yield* events.Finish("closed");
							if (command._tag === "steer") {
								if (prompt_active)
									return yield* new EngineUnsupportedCommandError({
										command: command._tag,
										command_id: command.command_id,
										engine_id: definition.descriptor.id,
									});
								yield* Effect.forkScoped(
									RunPrompt(command.text, command.content),
								).pipe(Scope.provide(run_scope));
								return;
							}
							if (command._tag === "respond_approval") {
								const pending = approvals.get(command.approval_id);
								if (pending === undefined)
									return yield* new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "approval",
										target_id: command.approval_id,
									});
								approvals.delete(command.approval_id);
								pending.Resolve(command.approved);
								return yield* events.Emit({
									...raw_base(
										command,
										`approval:${command.approval_id}:resolved`,
									),
									_tag: "approval",
									approval_id: command.approval_id,
									approved: command.approved,
									description: pending.description,
									request: pending.request,
									state: "resolved",
								});
							}
							const pending_entry = questions.entries().next().value as
								| [string, PendingQuestion]
								| undefined;
							if (pending_entry === undefined)
								return yield* new EngineCommandTargetError({
									artisan_run_id: input.artisan_run_id,
									command_id: command.command_id,
									target: "question",
									target_id: "pending",
								});
							questions.delete(pending_entry[0]);
							pending_entry[1].Resolve(command.answers);
							yield* Effect.forEach(pending_entry[1].question_ids, (question_id) =>
								events.Emit({
									...raw_base(command, `question:${question_id}:resolved`),
									_tag: "question",
									answers: command.answers[question_id] ?? [],
									question_id,
									state: "resolved",
									text: question_id,
								}),
							);
						}),
					);

				return {
					artisan_run_id: input.artisan_run_id,
					Closed: events.Closed,
					Events: events.Events,
					native_thread_id,
					resume_token: { native_thread_id },
					Send,
				} satisfies EngineRun;
			}).pipe(
				Effect.mapError(
					(failure): EngineFailure =>
						failure instanceof EngineConfigurationError ||
						failure instanceof EngineProcessError ||
						failure instanceof EngineProtocolError ||
						failure instanceof EngineUnavailableError
							? failure
							: new EngineProtocolError({
									engine_id: definition.descriptor.id,
									message: String(failure),
								}),
				),
			);

		return {
			Descriptor: definition.descriptor,
			Open,
			Probe,
			...(definition.Usage === undefined ? {} : { Usage: definition.Usage }),
		} satisfies Engine;
	});
