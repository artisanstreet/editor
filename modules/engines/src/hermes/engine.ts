import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";

import { Context, Effect, Exit, Layer, Queue, Ref, Scope, Semaphore, Stream } from "effect";

import {
	EngineCommandIdConflictError,
	EngineCommandTargetError,
	EngineConfigurationError,
	EngineProtocolError,
	EngineRunClosedError,
	EngineUnavailableError,
	ValidateEngineGlobalGuidance,
	ValidateEngineProductInstructions,
	type Engine,
	type EngineCatalogModel,
	type EngineCatalogRoute,
	type EngineCatalogScope,
	type EngineCommand,
	type EngineCommandFailure,
	type EngineDescriptor,
	type EngineFailure,
	type EngineModelCatalogSnapshot,
	type EngineObservation,
	type EngineObservationBase,
	type EngineOpenInput,
	type EngineProbe,
	type EngineRun,
	type EngineUserInputPart,
} from "../engine";
import { MakeEngineEventBuffer } from "../process/event-buffer";
import { EngineProcessFactory } from "../process/process";
import {
	type ResolveEngineSpawnOverride,
	with_engine_spawn_override,
} from "../process/spawn-override";
import {
	DecodeHermesApproval,
	DecodeHermesQuestions,
	HermesEventNormalizer,
	type HermesPendingApproval,
	type HermesPendingQuestion,
} from "./normalizer";
import { HermesGatewayError, type HermesGatewayClient, type HermesGatewayEvent } from "./protocol";
import {
	StartHermesPrivateService,
	type HermesPrivateService,
	type HermesPrivateServiceOptions,
} from "./service";

export const HermesEngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "supported",
			reason: "Hermes gateway approval requests map to one-shot Artisan approvals.",
		},
		auth: {
			state: "unsupported",
			reason: "Provider setup remains owned by the installed Hermes profile.",
		},
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: {
			state: "supported",
			reason: "The private Hermes WebSocket streams messages, tools, questions, and usage.",
		},
		global_guidance: {
			state: "experimental",
			reason: "Guidance is seeded as a system history entry on new Hermes sessions.",
		},
		model_catalog: {
			state: "supported",
			reason: "Authenticated Hermes providers and models come from model.options.",
		},
		model_selection: { state: "supported" },
		native_continuation: {
			state: "experimental",
			reason: "Hermes durable sessions resume only with their original model selection.",
		},
		native_tools: { state: "supported" },
		probe: { state: "supported" },
		question: { state: "supported" },
		raw_frames: {
			state: "supported",
			reason: "Only event identity is retained; prompts, tool arguments, and results are excluded.",
		},
		resume: { state: "experimental" },
		start: { state: "supported" },
		steer: { state: "supported" },
		subagents: {
			state: "experimental",
			reason: "Hermes subagent lifecycle is projected without adopting child sessions.",
		},
	},
	display_name: "Hermes",
	id: "hermes",
	transport: "hermes-jsonrpc-websocket",
};

export interface HermesEngineOptions {
	readonly ResolveSpawnOverride?: ResolveEngineSpawnOverride;
	readonly StartService?: (
		options: HermesPrivateServiceOptions,
	) => Effect.Effect<HermesPrivateService, EngineFailure, Scope.Scope>;
}

export class HermesEngine extends Context.Service<HermesEngine, Engine>()("Artisan/HermesEngine") {}

type RecordValue = Readonly<Record<string, unknown>>;

const record = (value: unknown): RecordValue | undefined =>
	typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as RecordValue)
		: undefined;
const string = (value: unknown) => (typeof value === "string" ? value : undefined);
const number = (value: unknown) =>
	typeof value === "number" && Number.isFinite(value) ? value : undefined;
const boolean = (value: unknown) => (typeof value === "boolean" ? value : undefined);

const api_failure = (failure: HermesGatewayError): EngineFailure =>
	new EngineProtocolError({ engine_id: "hermes", message: failure.message });

const MapApi = <A, R>(effect: Effect.Effect<A, HermesGatewayError, R>) =>
	effect.pipe(Effect.mapError(api_failure));

const catalog_id = (provider_route_id: string, model_id: string) =>
	`hermes:${Buffer.from(JSON.stringify({ model_id, provider_route_id })).toString("base64url")}`;

/**
 * Hermes reports canonical provider model identifiers rather than display
 * names. Preserve their casing and separators; only omit a redundant vendor
 * prefix because the provider route is already shown beside the model.
 */
const model_name = (model_id: string) => model_id.split("/").at(-1) ?? model_id;

const formatted_price = (value: unknown) => {
	if (value === "free") return 0;
	if (typeof value !== "string") return undefined;
	const parsed = Number(value.replace(/[^0-9.-]/g, ""));
	return Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined;
};

interface HermesCatalogProvider {
	readonly authenticated: boolean;
	readonly capabilities: ReadonlyMap<string, RecordValue>;
	readonly models: ReadonlyArray<string>;
	readonly name: string;
	readonly order: number;
	readonly pricing: ReadonlyMap<string, RecordValue>;
	readonly slug: string;
	readonly unavailable_models: ReadonlySet<string>;
}

const DecodeCatalogProviders = (value: unknown): ReadonlyArray<HermesCatalogProvider> => {
	const payload = record(value);
	if (!Array.isArray(payload?.providers)) return [];
	return payload.providers.flatMap((item, order) => {
		const provider = record(item);
		const slug = string(provider?.slug);
		const name = string(provider?.name);
		if (slug === undefined || name === undefined || !Array.isArray(provider?.models)) return [];
		const models = provider.models.filter(
			(model): model is string => typeof model === "string" && model.trim().length > 0,
		);
		const capability_record = record(provider.capabilities) ?? {};
		const pricing_record = record(provider.pricing) ?? {};
		return [
			{
				authenticated: boolean(provider.authenticated) ?? true,
				capabilities: new Map(
					Object.entries(capability_record).flatMap(([model, capability]) => {
						const decoded = record(capability);
						return decoded === undefined ? [] : [[model, decoded] as const];
					}),
				),
				models,
				name,
				order,
				pricing: new Map(
					Object.entries(pricing_record).flatMap(([model, price]) => {
						const decoded = record(price);
						return decoded === undefined ? [] : [[model, decoded] as const];
					}),
				),
				slug,
				unavailable_models: new Set(
					Array.isArray(provider.unavailable_models)
						? provider.unavailable_models.filter(
								(model): model is string => typeof model === "string",
							)
						: [],
				),
			},
		];
	});
};

/** Converts model.options into the route-aware catalog consumed by the shared selector. */
export const HermesCatalogFromOptions = (
	value: unknown,
	scope: EngineCatalogScope,
): EngineModelCatalogSnapshot => {
	const providers = DecodeCatalogProviders(value);
	const routes: ReadonlyArray<EngineCatalogRoute> = providers.map((provider) => ({
		group: {
			id: provider.slug,
			label: provider.name,
			order: provider.order,
			show_route_labels: false,
		},
		id: provider.slug,
		label: provider.name,
		status: provider.authenticated ? "available" : "unavailable",
		...(provider.authenticated
			? {}
			: { unavailable_reason: `${provider.name} is not authenticated in Hermes.` }),
	}));
	const models: ReadonlyArray<EngineCatalogModel> = providers.flatMap((provider) =>
		provider.models.map((model_id) => {
			const capability = provider.capabilities.get(model_id);
			const fast = boolean(capability?.fast);
			const reasoning = boolean(capability?.reasoning);
			const price = provider.pricing.get(model_id);
			const input_price = formatted_price(price?.input);
			const output_price = formatted_price(price?.output);
			return {
				capabilities: {
					...(fast === undefined ? {} : { fast }),
					image_input: false,
					...(reasoning === undefined ? {} : { reasoning }),
					tools: true,
				},
				catalog_id: catalog_id(provider.slug, model_id),
				...(input_price === undefined && output_price === undefined
					? {}
					: {
							cost: {
								...(input_price === undefined
									? {}
									: { input_usd_per_million: input_price }),
								...(output_price === undefined
									? {}
									: { output_usd_per_million: output_price }),
							},
						}),
				enabled: provider.authenticated && !provider.unavailable_models.has(model_id),
				metadata_confidence: capability === undefined ? "unknown" : "reported",
				model_id,
				name: model_name(model_id),
				provider_route_id: provider.slug,
				status: "active",
				upstream_model_id: model_id,
			};
		}),
	);
	const revision = createHash("sha256").update(JSON.stringify({ models, routes })).digest("hex");
	return {
		engine_id: "hermes",
		generated_at: new Date().toISOString(),
		models,
		revision,
		routes,
		scope,
	};
};

const CatalogFrom = (client: HermesGatewayClient, scope: EngineCatalogScope) =>
	MapApi(
		client.Request("model.options", {
			explicit_only: true,
			include_unconfigured: false,
			refresh: false,
		}),
	).pipe(Effect.map((value) => HermesCatalogFromOptions(value, scope)));

const command_intent = (command: EngineCommand) => {
	switch (command._tag) {
		case "cancel":
		case "close":
			return command._tag;
		case "steer":
			return JSON.stringify([command._tag, command.text, command.content]);
		case "respond_approval":
			return JSON.stringify([command._tag, command.approval_id, command.approved]);
		case "respond_question":
			return JSON.stringify([command._tag, command.answers]);
	}
};

const interaction_base = (
	artisan_run_id: string,
	native_thread_id: string,
	native_id: string,
	method: string,
): EngineObservationBase => ({
	artisan_run_id,
	native_thread_id,
	observation_id: `${artisan_run_id}:hermes:${method}:${native_id}`,
	raw: {
		engine_id: "hermes",
		frame: { native_id, type: method },
		native_id,
		native_method: method,
		protocol_version: "hermes-desktop-contract-6",
		transport: "hermes-jsonrpc-websocket",
	},
	sequence: 0,
});

const guidance_messages = (input: EngineOpenInput) => {
	const sections = [
		input.product_instructions === undefined
			? undefined
			: `Artisan product instructions (${input.product_instructions.source}):\n${input.product_instructions.content}`,
		input.global_guidance === undefined
			? undefined
			: `Workspace guidance (${input.global_guidance.source_file}):\n${input.global_guidance.content}`,
	].filter((section): section is string => section !== undefined);
	return sections.length === 0 ? [] : [{ role: "system", content: sections.join("\n\n") }];
};

const user_turn = (fallback: string, content: ReadonlyArray<EngineUserInputPart> | undefined) => {
	const text =
		content?.flatMap((part) => (part.type === "text" ? [part.text] : [])).join("\n") ||
		fallback;
	const images = content?.flatMap((part) => (part.type === "image" ? [part] : [])) ?? [];
	return { images, text };
};

interface HermesRunState {
	readonly approvals: ReadonlyMap<string, HermesPendingApproval>;
	readonly command_intents: ReadonlyMap<string, string>;
	readonly questions: ReadonlyMap<string, HermesPendingQuestion>;
}

const is_nonempty_string = (value: unknown): value is string =>
	typeof value === "string" && value.trim().length > 0;

const MakeHermesEngine = (
	factory: typeof EngineProcessFactory.Service,
	StartService: NonNullable<HermesEngineOptions["StartService"]>,
): Engine => {
	const ServiceFor = (scope: EngineCatalogScope) =>
		Effect.scoped(
			Effect.gen(function* () {
				if (scope.workspace_trust !== "safe")
					return yield* new EngineConfigurationError({
						engine_id: "hermes",
						option: "workspace_trust",
						value: scope.workspace_trust,
					});
				return yield* StartService({
					factory,
					profile_id: scope.profile_id,
					working_directory: scope.working_directory,
				});
			}),
		);

	const Catalog = (scope: EngineCatalogScope) =>
		Effect.scoped(
			Effect.gen(function* () {
				const service = yield* ServiceFor(scope);
				return yield* CatalogFrom(service.Client, scope);
			}),
		);

	const Probe = (): Effect.Effect<EngineProbe, EngineFailure> =>
		Effect.scoped(
			Effect.gen(function* () {
				const scope = {
					profile_id: "default",
					working_directory: process.cwd(),
					workspace_trust: "safe" as const,
				};
				const service = yield* ServiceFor(scope);
				const catalog = yield* CatalogFrom(service.Client, scope);
				return {
					authentication: {
						state: catalog.models.some((model) => model.enabled)
							? ("authenticated" as const)
							: ("unauthenticated" as const),
					},
					capabilities: HermesEngineDescriptor.capabilities,
					descriptor: HermesEngineDescriptor,
					metadata: { desktop_contract: "6" },
					ready: true,
					version: service.version,
				};
			}),
		);

	const Open = (input: EngineOpenInput): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> =>
		Effect.gen(function* () {
			yield* ValidateEngineGlobalGuidance("hermes", input.global_guidance);
			yield* ValidateEngineProductInstructions("hermes", input.product_instructions);
			const profile_id = input.profile_id;
			const provider_route_id = input.provider_route_id;
			const model_id = input.model_id ?? input.model;
			for (const [option, value] of [
				["profile_id", profile_id],
				["provider_route_id", provider_route_id],
				["model_id", model_id],
				["catalog_revision", input.catalog_revision],
			] as const) {
				if (!is_nonempty_string(value))
					return yield* new EngineConfigurationError({
						engine_id: "hermes",
						option,
						value,
					});
			}
			const selected_profile_id = profile_id as string;
			const selected_provider_route_id = provider_route_id as string;
			const selected_model_id = model_id as string;
			const selected_catalog_revision = input.catalog_revision as string;
			const permission_mode = input.provider_options?.["hermes.permission_mode"];
			if (permission_mode !== "profile" && permission_mode !== "yolo")
				return yield* new EngineConfigurationError({
					engine_id: "hermes",
					option: "provider_options.hermes.permission_mode",
					value: permission_mode,
				});

			const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
				Scope.close(scope, Exit.void),
			);
			const service = yield* StartService({
				factory,
				profile_id: selected_profile_id,
				working_directory: input.working_directory,
			}).pipe(Scope.provide(run_scope));
			const client = service.Client;
			const catalog_scope = {
				profile_id: selected_profile_id,
				working_directory: input.working_directory,
				workspace_trust: "safe" as const,
			};
			const live_catalog = yield* CatalogFrom(client, catalog_scope);
			if (
				selected_catalog_revision !== live_catalog.revision &&
				!selected_catalog_revision.split("+").includes(`hermes:${live_catalog.revision}`)
			)
				return yield* new EngineConfigurationError({
					engine_id: "hermes",
					option: "catalog_revision",
					value: selected_catalog_revision,
				});
			if (
				!live_catalog.models.some(
					(model) =>
						model.enabled &&
						model.model_id === selected_model_id &&
						model.provider_route_id === selected_provider_route_id,
				)
			)
				return yield* new EngineConfigurationError({
					engine_id: "hermes",
					option: "model_selection",
					value: {
						model_id: selected_model_id,
						provider_route_id: selected_provider_route_id,
					},
				});

			const profile = selected_profile_id === "default" ? undefined : selected_profile_id;
			const response = yield* MapApi(
				input._tag === "start"
					? client.Request("session.create", {
							close_on_disconnect: false,
							cwd: input.working_directory,
							fast: input.provider_options?.["hermes.fast"] === true,
							messages: guidance_messages(input),
							model: selected_model_id,
							...(profile === undefined ? {} : { profile }),
							provider: selected_provider_route_id,
							reasoning_effort:
								string(input.provider_options?.["hermes.reasoning_effort"]) ??
								"medium",
							source: "artisan",
						})
					: client.Request("session.resume", {
							close_on_disconnect: false,
							defer_history: true,
							omit_messages: true,
							...(profile === undefined ? {} : { profile }),
							session_id: input.resume_token.native_thread_id,
							source: "artisan",
						}),
			);
			const response_record = record(response);
			const runtime_session_id = string(response_record?.session_id);
			if (runtime_session_id === undefined)
				return yield* new EngineProtocolError({
					engine_id: "hermes",
					message: "Hermes did not return a runtime session identifier.",
				});
			const info = record(response_record?.info);
			const desktop_contract = number(info?.desktop_contract);
			if (desktop_contract !== undefined && desktop_contract < 4)
				return yield* new EngineUnavailableError({
					engine_id: "hermes",
					message: `Hermes desktop contract ${desktop_contract} is older than the required contract 4.`,
				});
			if (input._tag === "resume") {
				const resumed_model = string(info?.model);
				const resumed_provider = string(info?.provider);
				if (
					(resumed_model !== undefined && resumed_model !== selected_model_id) ||
					(resumed_provider !== undefined &&
						resumed_provider !== selected_provider_route_id)
				)
					return yield* new EngineConfigurationError({
						engine_id: "hermes",
						option: "resume_token.model_selection",
						value: { model: resumed_model, provider: resumed_provider },
					});
			}
			const native_thread_id =
				input._tag === "resume"
					? input.resume_token.native_thread_id
					: (string(response_record?.stored_session_id) ??
						string(response_record?.session_key));
			if (native_thread_id === undefined)
				return yield* new EngineProtocolError({
					engine_id: "hermes",
					message: "Hermes did not return a durable session identifier.",
				});

			const normalizer = new HermesEventNormalizer({
				artisan_run_id: input.artisan_run_id,
				native_thread_id,
				provider_route_id: selected_provider_route_id,
			});
			const state = yield* Ref.make<HermesRunState>({
				approvals: new Map(),
				command_intents: new Map(),
				questions: new Map(),
			});
			const command_lock = yield* Semaphore.make(1);
			let unsubscribe: () => void = () => undefined;
			const CloseRun = Effect.gen(function* () {
				unsubscribe();
				yield* client
					.Request("session.close", { session_id: runtime_session_id })
					.pipe(Effect.timeout("5 seconds"), Effect.ignore);
				yield* service.Close;
			});
			const event_buffer = yield* MakeEngineEventBuffer({
				artisan_run_id: input.artisan_run_id,
				CloseResource: CloseRun,
				make_terminal_observation: (terminal_state, sequence, error_ref) => ({
					_tag: "run_terminal",
					artisan_run_id: input.artisan_run_id,
					...(error_ref === undefined ? {} : { error_ref }),
					native_thread_id,
					observation_id: `${input.artisan_run_id}:hermes:terminal:${sequence}`,
					raw: {
						engine_id: "hermes",
						frame: { source: "event-buffer", state: terminal_state },
						protocol_version: "hermes-desktop-contract-6",
						transport: "hermes-jsonrpc-websocket",
					},
					sequence,
					state: terminal_state,
				}),
			});
			const Emit = event_buffer.Emit;
			const Finish = event_buffer.Finish;
			const ProcessObservation = (observation: EngineObservation) =>
				observation._tag === "run_terminal"
					? Finish(observation.state, observation.error_ref)
					: Emit(observation);
			const ProcessEvent = (event: HermesGatewayEvent) =>
				Effect.gen(function* () {
					const approval = DecodeHermesApproval(event);
					if (approval !== undefined)
						yield* Ref.update(state, (current) => ({
							...current,
							approvals: new Map(current.approvals).set(approval.id, approval),
						}));
					const questions = DecodeHermesQuestions(event);
					if (questions.length > 0)
						yield* Ref.update(state, (current) => {
							const pending = new Map(current.questions);
							for (const question of questions) pending.set(question.id, question);
							return { ...current, questions: pending };
						});
					yield* Effect.forEach(normalizer.Normalize(event), ProcessObservation, {
						discard: true,
					});
				});
			const frames = yield* Queue.unbounded<HermesGatewayEvent>();
			unsubscribe = client.Subscribe((event) => {
				if (event.session_id !== runtime_session_id) return;
				Effect.runFork(Queue.offer(frames, event));
			});
			yield* Scope.addFinalizer(run_scope, Effect.sync(unsubscribe));
			yield* Stream.fromQueue(frames).pipe(
				Stream.runForEach(ProcessEvent),
				Effect.forkScoped,
				Scope.provide(run_scope),
			);
			yield* service.Closed.pipe(
				Effect.andThen(event_buffer.IsClosed),
				Effect.flatMap((closed) => (closed ? Effect.void : Finish("interrupted"))),
				Effect.forkScoped,
				Scope.provide(run_scope),
			);
			yield* Emit({
				...interaction_base(input.artisan_run_id, native_thread_id, "opening", "run.state"),
				_tag: "run_state",
				state: "opening",
			});

			yield* MapApi(
				client.Request("config.set", {
					key: "yolo",
					scope: "session",
					session_id: runtime_session_id,
					value: permission_mode === "yolo" ? "on" : "off",
				}),
			);
			const source_turn =
				input._tag === "start"
					? user_turn(input.initial_text, input.initial_content)
					: user_turn(
							input.next_text ?? "Continue this Hermes session.",
							input.next_content,
						);
			for (const image of source_turn.images)
				yield* MapApi(
					client.Request("image.attach_bytes", {
						content_base64: Buffer.from(image.bytes).toString("base64"),
						filename: image.name,
						session_id: runtime_session_id,
					}),
				);
			yield* MapApi(
				client.Request("prompt.submit", {
					session_id: runtime_session_id,
					text: source_turn.text,
				}),
			);
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
						const prior = current.command_intents.get(command.command_id);
						if (prior !== undefined) {
							if (prior === intent) return;
							return yield* new EngineCommandIdConflictError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							});
						}
						if (yield* event_buffer.IsClosed) {
							if (command._tag === "close") return;
							return yield* new EngineRunClosedError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							});
						}
						switch (command._tag) {
							case "cancel":
								yield* MapApi(
									client.Request("session.interrupt", {
										session_id: runtime_session_id,
									}),
								);
								yield* RememberCommand(command.command_id, intent);
								yield* Finish("cancelled");
								return;
							case "close":
								yield* RememberCommand(command.command_id, intent);
								yield* Finish("closed");
								return;
							case "steer":
								yield* MapApi(
									client.Request("session.steer", {
										session_id: runtime_session_id,
										text: command.text,
									}),
								);
								break;
							case "respond_approval": {
								const approval = current.approvals.get(command.approval_id);
								if (approval === undefined)
									return yield* new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "approval",
										target_id: command.approval_id,
									});
								yield* MapApi(
									client.Request("approval.respond", {
										choice: command.approved ? "once" : "deny",
										request_id: approval.id,
										session_id: runtime_session_id,
									}),
								);
								yield* Ref.update(state, (next) => {
									const approvals = new Map(next.approvals);
									approvals.delete(approval.id);
									return { ...next, approvals };
								});
								yield* Emit({
									...interaction_base(
										input.artisan_run_id,
										native_thread_id,
										approval.id,
										"approval.respond",
									),
									_tag: "approval",
									approval_id: approval.id,
									approved: command.approved,
									description: approval.description,
									request: {
										...(approval.command === undefined
											? {}
											: { command: approval.command }),
										kind: approval.command === undefined ? "action" : "command",
									},
									state: "resolved",
								});
								break;
							}
							case "respond_question": {
								for (const [question_id, answers] of Object.entries(
									command.answers,
								)) {
									const question = current.questions.get(question_id);
									if (question === undefined)
										return yield* new EngineCommandTargetError({
											artisan_run_id: input.artisan_run_id,
											command_id: command.command_id,
											target: "question",
											target_id: question_id,
										});
									yield* MapApi(
										client.Request("clarify.respond", {
											answer: answers.join(", "),
											...(question.question_id === undefined
												? {}
												: { question_id: question.question_id }),
											request_id: question.request_id,
											session_id: runtime_session_id,
										}),
									);
									yield* Emit({
										...interaction_base(
											input.artisan_run_id,
											native_thread_id,
											question.id,
											"clarify.respond",
										),
										_tag: "question",
										answers,
										multi_select: question.multi_select,
										...(question.options === undefined
											? {}
											: {
													options: question.options.map((label) => ({
														label,
													})),
												}),
										question_id: question.id,
										state: "resolved",
										text: question.text,
									});
									yield* Ref.update(state, (next) => {
										const questions = new Map(next.questions);
										questions.delete(question.id);
										return { ...next, questions };
									});
								}
								break;
							}
						}
						yield* RememberCommand(command.command_id, intent);
					}),
				).pipe(Effect.mapError((failure) => failure as EngineCommandFailure));

			return {
				artisan_run_id: input.artisan_run_id,
				Closed: event_buffer.Closed,
				Events: event_buffer.Events,
				native_thread_id,
				resume_token: {
					native_thread_id,
					opaque_checkpoint: "hermes:stored-session-v1",
				},
				Send,
			};
		});

	return {
		Catalog,
		CheckNativeContinuation: (continuation) =>
			Effect.succeed(
				continuation.source_model === undefined ||
					continuation.target_model === undefined ||
					continuation.source_model === continuation.target_model
					? ({ state: "compatible" } as const)
					: ({
							reason: "Hermes resumes the provider and model stored with its native session.",
							state: "incompatible",
						} as const),
			),
		Descriptor: HermesEngineDescriptor,
		Open,
		Probe,
	};
};

export const make_hermes_engine_layer = (
	options: HermesEngineOptions = {},
): Layer.Layer<HermesEngine, never, EngineProcessFactory> =>
	Layer.effect(
		HermesEngine,
		Effect.gen(function* () {
			const base_factory = yield* EngineProcessFactory;
			const factory =
				options.ResolveSpawnOverride === undefined
					? base_factory
					: with_engine_spawn_override(base_factory, options.ResolveSpawnOverride);
			const service_scope = yield* Scope.Scope;
			const pool_lock = yield* Semaphore.make(1);
			const services = yield* Ref.make<ReadonlyMap<string, HermesPrivateService>>(new Map());
			const PooledStart: NonNullable<HermesEngineOptions["StartService"]> = (input) =>
				Semaphore.withPermit(pool_lock)(
					Effect.gen(function* () {
						const existing = (yield* Ref.get(services)).get(input.profile_id);
						if (existing !== undefined) {
							if (existing.Client.IsOpen())
								return { ...existing, Close: Effect.void };
							yield* existing.Close;
							yield* Ref.update(services, (current) => {
								const next = new Map(current);
								next.delete(input.profile_id);
								return next;
							});
						}
						const service = yield* StartHermesPrivateService({
							...input,
							factory,
						}).pipe(Scope.provide(service_scope));
						yield* Ref.update(services, (current) =>
							new Map(current).set(input.profile_id, service),
						);
						return { ...service, Close: Effect.void };
					}),
				);
			return MakeHermesEngine(factory, options.StartService ?? PooledStart);
		}),
	);
