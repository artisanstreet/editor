import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import { resolve } from "node:path";

import { Context, Deferred, Effect, Exit, Layer, Ref, Scope, Semaphore, Stream } from "effect";

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
import { opencode2_certified_upstream_commit } from "../toolchain/distribution";
import { MakeOpenCode2Connections } from "./connections";
import {
	DecodeOpenCode2PendingForm,
	DecodeOpenCode2PendingPermission,
	IsOpenCode2DurableEvent,
	OpenCode2DurableSequence,
	OpenCode2EventDeduplicationKey,
	OpenCode2EventNormalizer,
	OpenCode2EventSessionId,
	OpenCode2FormAnswer,
	RecoverOpenCode2Projection,
	type OpenCode2PendingForm,
	type OpenCode2PendingPermission,
} from "./normalizer";
import { OpenCode2ApiError, type OpenCode2ApiClient, type OpenCode2Model } from "./protocol";
import {
	StartOpenCode2PrivateService,
	type OpenCode2PrivateService,
	type OpenCode2PrivateServiceOptions,
} from "./service";

export const OpenCode2EngineDescriptor: EngineDescriptor = {
	capabilities: {
		approval: {
			state: "supported",
			reason: "Pending V2 permission requests map to one-shot approvals.",
		},
		auth: {
			state: "experimental",
			reason: "Authentication is owned by the V2 integration API.",
		},
		cancel: { state: "supported" },
		close: { state: "supported" },
		events: {
			state: "supported",
			reason: "Live durable envelopes supply low-latency settlement; bounded replay and message projection recover missed events.",
		},
		global_guidance: { state: "supported" },
		model_catalog: {
			state: "supported",
			reason: "Models are read from the live location-scoped V2 catalog.",
		},
		model_selection: { state: "supported" },
		native_continuation: {
			state: "experimental",
			reason: "Private-service restart continuation is source-pinned to the certified beta.",
		},
		native_tools: { state: "experimental" },
		probe: { state: "supported" },
		question: { state: "supported", reason: "V2 forms map to canonical questions." },
		raw_frames: {
			state: "supported",
			reason: "Only sanitized event identity is retained; prompt and tool payloads are excluded.",
		},
		resume: { state: "experimental" },
		start: { state: "supported" },
		steer: { state: "supported" },
		subagents: {
			state: "unsupported",
			reason: "Native subagents are denied until child permissions are proven not to widen the parent.",
		},
	},
	display_name: "OpenCode",
	id: "opencode2",
	transport: "opencode2-http-sse",
};

export interface OpenCode2EngineOptions {
	readonly ResolveSpawnOverride?: ResolveEngineSpawnOverride;
	readonly StartService?: (
		options: OpenCode2PrivateServiceOptions,
	) => Effect.Effect<OpenCode2PrivateService, EngineFailure, Scope.Scope>;
}

export class OpenCode2Engine extends Context.Service<OpenCode2Engine, Engine>()(
	"Artisan/OpenCode2Engine",
) {}

interface OpenCode2RunState {
	readonly cancel_requested: boolean;
	readonly command_intents: ReadonlyMap<string, string>;
	readonly forms: ReadonlyMap<string, OpenCode2PendingForm>;
	readonly permissions: ReadonlyMap<string, OpenCode2PendingPermission>;
}

const observation_failure = {
	artisan_code: "AE-RUN-301",
	detail: "OpenCode completed without a recoverable terminal observation.",
} as const;

const api_failure = (failure: OpenCode2ApiError): EngineFailure =>
	new EngineProtocolError({
		engine_id: "opencode2",
		message:
			failure.code === "http"
				? `OpenCode rejected ${failure.operation} with HTTP ${String(failure.status ?? "error")}.`
				: `OpenCode ${failure.operation} failed its ${failure.code} boundary.`,
	});

const MapApi = <A, R>(effect: Effect.Effect<A, OpenCode2ApiError, R>) =>
	effect.pipe(Effect.mapError(api_failure));

const deterministic_id = (prefix: "msg" | "obs", value: string) =>
	`${prefix}_artisan_${createHash("sha256").update(value).digest("hex").slice(0, 32)}`;

const catalog_id = (model: OpenCode2Model, variant_id?: string) =>
	`opencode2:${Buffer.from(
		JSON.stringify({
			model_id: model.id,
			provider_route_id: model.providerID,
			...(variant_id === undefined ? {} : { variant_id }),
		}),
	).toString("base64url")}`;

const known_routes: Readonly<Record<string, Pick<EngineCatalogRoute, "group" | "label">>> = {
	"opencode-go": {
		group: { id: "go", label: "Go", order: 0, show_route_labels: false },
		label: "Go",
	},
	opencode: {
		group: { id: "zen", label: "Zen", order: 1, show_route_labels: false },
		label: "Zen",
	},
};

const catalog_routes = (
	models: ReadonlyArray<OpenCode2Model>,
): ReadonlyArray<EngineCatalogRoute> => {
	const routes = new Map<string, EngineCatalogRoute>();
	for (const model of models) {
		if (routes.has(model.providerID)) continue;
		const known = known_routes[model.providerID];
		routes.set(model.providerID, {
			group: known?.group ?? {
				id: "custom",
				label: "Custom",
				order: 2,
				show_route_labels: true,
			},
			id: model.providerID,
			label: known?.label ?? model.providerID,
			status: "available",
		});
	}
	return [...routes.values()].sort(
		(left, right) =>
			left.group.order - right.group.order || left.label.localeCompare(right.label),
	);
};

const catalog_models = (models: ReadonlyArray<OpenCode2Model>): ReadonlyArray<EngineCatalogModel> =>
	models.flatMap((model) => {
		const variants: ReadonlyArray<string | undefined> = [
			undefined,
			...model.variants.map((variant) => variant.id),
		];
		return variants.map((variant_id) => ({
			capabilities: {
				context_window_tokens: model.limit.context,
				image_input: model.capabilities.input.includes("image"),
				output_tokens: model.limit.output,
				tools: model.capabilities.tools,
			},
			catalog_id: catalog_id(model, variant_id),
			...(model.cost[0] === undefined
				? {}
				: {
						cost: {
							input_usd_per_million: model.cost[0].input,
							output_usd_per_million: model.cost[0].output,
						},
					}),
			enabled: model.enabled,
			metadata_confidence: "reported",
			model_id: model.id,
			name: model.name,
			provider_route_id: model.providerID,
			status: model.status,
			upstream_model_id: model.modelID,
			...(variant_id === undefined ? {} : { variant_id }),
		}));
	});

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

const prompt_parts = (text: string, content: ReadonlyArray<EngineUserInputPart> | undefined) => {
	if (content === undefined) return { text };
	const text_parts = content.flatMap((part) => (part.type === "text" ? [part.text] : []));
	const files = content.flatMap((part) =>
		part.type === "image"
			? [
					{
						name: part.name,
						uri: `data:${part.media_type};base64,${Buffer.from(part.bytes).toString("base64")}`,
					},
				]
			: [],
	);
	return {
		...(files.length === 0 ? {} : { files }),
		text: text_parts.length === 0 ? text : text_parts.join("\n"),
	};
};

const same_directory = (left: string, right: string) =>
	process.platform === "win32"
		? resolve(left).toLowerCase() === resolve(right).toLowerCase()
		: resolve(left) === resolve(right);

const is_nonempty_string = (value: unknown): value is string =>
	typeof value === "string" && value.trim().length > 0;

const latest_sequence = (client: OpenCode2ApiClient, session_id: string) =>
	client.SessionLog(session_id, undefined, false).pipe(
		Stream.runFold(
			() => undefined as number | undefined,
			(latest, event) => {
				const envelope =
					typeof event === "object" && event !== null && !Array.isArray(event)
						? (event as Readonly<Record<string, unknown>>)
						: undefined;
				if (envelope?.type === "log.synced") {
					return typeof envelope.seq === "number" ? envelope.seq : latest;
				}
				return OpenCode2DurableSequence(event) ?? latest;
			},
		),
		Effect.mapError(api_failure),
	);

const interaction_base = (
	artisan_run_id: string,
	native_thread_id: string,
	native_id: string,
	method: string,
): EngineObservationBase => ({
	artisan_run_id,
	native_thread_id,
	observation_id: `${artisan_run_id}:opencode2:${method}:${native_id}`,
	raw: {
		engine_id: "opencode2",
		frame: { native_id, type: method },
		native_id,
		native_method: method,
		protocol_version: "opencode-v2-0.0.1",
		transport: "opencode2-http-sse",
	},
	sequence: 0,
});

const option_records = (value: unknown): ReadonlyArray<Readonly<Record<string, unknown>>> =>
	Array.isArray(value)
		? value.flatMap((item) =>
				typeof item === "object" && item !== null && !Array.isArray(item)
					? [item as Readonly<Record<string, unknown>>]
					: [],
			)
		: [];

const MakeOpenCode2Engine = (
	factory: typeof EngineProcessFactory.Service,
	StartService: NonNullable<OpenCode2EngineOptions["StartService"]>,
): Engine => {
	const ServiceFor = (scope: EngineCatalogScope) =>
		Effect.scoped(
			Effect.gen(function* () {
				if (scope.workspace_trust !== "safe")
					return yield* new EngineConfigurationError({
						engine_id: "opencode2",
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
				const source_models = yield* MapApi(service.Client.Models(scope));
				const models = catalog_models(source_models);
				const routes = catalog_routes(source_models);
				const revision = createHash("sha256")
					.update(JSON.stringify({ models, routes }))
					.digest("hex");
				return {
					engine_id: "opencode2",
					generated_at: new Date().toISOString(),
					models,
					revision,
					routes,
					scope,
				};
			}),
		);

	const Connections = MakeOpenCode2Connections(ServiceFor);

	const Probe = (): Effect.Effect<EngineProbe, EngineFailure> =>
		Effect.scoped(
			Effect.gen(function* () {
				const service = yield* StartService({
					factory,
					profile_id: "default",
					working_directory: process.cwd(),
				});
				return {
					authentication: { state: "unknown" },
					capabilities: OpenCode2EngineDescriptor.capabilities,
					descriptor: OpenCode2EngineDescriptor,
					metadata: {
						upstream_commit: opencode2_certified_upstream_commit,
					},
					ready: true,
					version: service.version,
				};
			}),
		);

	const Open = (input: EngineOpenInput): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> =>
		Effect.gen(function* () {
			yield* ValidateEngineGlobalGuidance("opencode2", input.global_guidance);
			yield* ValidateEngineProductInstructions("opencode2", input.product_instructions);
			const profile_id = input.profile_id;
			const provider_route_id = input.provider_route_id;
			const model_id = input.model_id ?? input.model;
			if (!is_nonempty_string(profile_id))
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "profile_id",
					value: profile_id,
				});
			if (!is_nonempty_string(provider_route_id))
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "provider_route_id",
					value: provider_route_id,
				});
			if (!is_nonempty_string(model_id))
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "model_id",
					value: model_id,
				});
			if (!is_nonempty_string(input.catalog_revision))
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "catalog_revision",
					value: input.catalog_revision,
				});
			if (input.permission_policy?.approval === "always")
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "permission_policy.approval",
					value: "always",
				});
			const agent = input.provider_options?.["opencode2.agent"];
			if (typeof agent !== "string" || !agent.startsWith("artisan-v1-"))
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "provider_options.opencode2.agent",
					value: agent,
				});
			if (input.provider_options?.["opencode2.project_config"] !== false)
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "provider_options.opencode2.project_config",
					value: input.provider_options?.["opencode2.project_config"],
				});
			for (const guidance of [
				input.global_guidance?.content,
				input.product_instructions?.content,
			]) {
				if (
					guidance !== undefined &&
					Buffer.byteLength(JSON.stringify(guidance)) > 8 * 1024
				)
					return yield* new EngineConfigurationError({
						engine_id: "opencode2",
						option: "instruction_entry",
						value: "exceeds 8 KiB",
					});
			}

			const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
				Scope.close(scope, Exit.void),
			);
			const service = yield* StartService({
				factory,
				profile_id,
				working_directory: input.working_directory,
			}).pipe(Scope.provide(run_scope));
			const client = service.Client;
			const live_source_models = yield* MapApi(
				client.Models({
					profile_id,
					working_directory: input.working_directory,
					workspace_trust: "safe",
				}),
			);
			const live_models = catalog_models(live_source_models);
			const live_routes = catalog_routes(live_source_models);
			const live_revision = createHash("sha256")
				.update(JSON.stringify({ models: live_models, routes: live_routes }))
				.digest("hex");
			if (
				input.catalog_revision !== live_revision &&
				!input.catalog_revision.split("+").includes(`opencode2:${live_revision}`)
			)
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "catalog_revision",
					value: input.catalog_revision,
				});
			const selected_live_model = live_models.find(
				(candidate) =>
					candidate.enabled &&
					candidate.provider_route_id === provider_route_id &&
					candidate.model_id === model_id &&
					candidate.variant_id === input.variant_id,
			);
			if (selected_live_model === undefined)
				return yield* new EngineConfigurationError({
					engine_id: "opencode2",
					option: "model_selection",
					value: {
						model_id,
						provider_route_id,
						...(input.variant_id === undefined ? {} : { variant_id: input.variant_id }),
					},
				});
			const selection = {
				...(input.catalog_revision === undefined
					? {}
					: { catalog_revision: input.catalog_revision }),
				model_id,
				profile_id,
				provider_route_id,
				...(input.variant_id === undefined ? {} : { variant_id: input.variant_id }),
			};
			let log_after: number | undefined;
			const native_thread_id =
				input._tag === "start"
					? yield* MapApi(
							client.CreateSession({
								agent,
								location: { directory: input.working_directory },
								model: {
									id: model_id,
									providerID: provider_route_id,
									...(input.variant_id === undefined
										? {}
										: { variant: input.variant_id }),
								},
							}),
						)
					: input.resume_token.native_thread_id;
			if (input._tag === "resume") {
				const session = yield* MapApi(client.GetSession(native_thread_id));
				if (!same_directory(session.location.directory, input.working_directory))
					return yield* new EngineConfigurationError({
						engine_id: "opencode2",
						option: "resume_token.location",
						value: session.location.directory,
					});
				yield* MapApi(client.SwitchAgent(native_thread_id, agent));
				yield* MapApi(client.SwitchModel(native_thread_id, selection));
				log_after = yield* latest_sequence(client, native_thread_id);
			}

			if (input.product_instructions !== undefined)
				yield* MapApi(
					client.PutInstruction(
						native_thread_id,
						"artisan-product",
						input.product_instructions.content,
					),
				);
			if (input.global_guidance !== undefined)
				yield* MapApi(
					client.PutInstruction(
						native_thread_id,
						"artisan-global-guidance",
						input.global_guidance.content,
					),
				);

			const normalizer = new OpenCode2EventNormalizer({
				artisan_run_id: input.artisan_run_id,
				native_thread_id,
				provider_route_id,
				...(input.variant_id === undefined ? {} : { variant_id: input.variant_id }),
			});
			const state = yield* Ref.make<OpenCode2RunState>({
				cancel_requested: false,
				command_intents: new Map(),
				forms: new Map(),
				permissions: new Map(),
			});
			const command_lock = yield* Semaphore.make(1);
			const event_buffer = yield* MakeEngineEventBuffer({
				artisan_run_id: input.artisan_run_id,
				CloseResource: service.Close,
				make_terminal_observation: (terminal_state, sequence, terminal_error_ref) => ({
					_tag: "run_terminal",
					artisan_run_id: input.artisan_run_id,
					...(terminal_error_ref === undefined ? {} : { error_ref: terminal_error_ref }),
					native_thread_id,
					observation_id: `${input.artisan_run_id}:opencode2:terminal:${sequence}`,
					raw: {
						engine_id: "opencode2",
						frame: { state: terminal_state, source: "event-buffer" },
						protocol_version: "opencode-v2-0.0.1",
						transport: "opencode2-http-sse",
					},
					sequence,
					state: terminal_state,
				}),
			});
			const Emit = event_buffer.Emit;
			const Finish = event_buffer.Finish;
			const observed_events = yield* Ref.make<ReadonlySet<string>>(new Set());
			const ProcessObservation = (observation: EngineObservation) =>
				observation._tag === "run_terminal"
					? Finish(observation.state, observation.error_ref)
					: Emit(observation);
			const ProcessEvent = (event: unknown) =>
				Effect.gen(function* () {
					const key = OpenCode2EventDeduplicationKey(event);
					if (key !== undefined) {
						const fresh = yield* Ref.modify(observed_events, (current) =>
							current.has(key)
								? ([false, current] as const)
								: ([true, new Set(current).add(key)] as const),
						);
						if (!fresh) return;
					}
					yield* Effect.forEach(normalizer.Normalize(event), ProcessObservation, {
						discard: true,
					});
				});
			const durable_cursor = yield* Ref.make(log_after);
			const ProcessDurableEvent = (event: unknown) =>
				Effect.gen(function* () {
					yield* ProcessEvent(event);
					const sequence = OpenCode2DurableSequence(event);
					if (sequence !== undefined)
						yield* Ref.update(durable_cursor, (current) =>
							Math.max(current ?? -1, sequence),
						);
				});

			const EmitPermission = (permission: OpenCode2PendingPermission) =>
				Effect.gen(function* () {
					const existing = (yield* Ref.get(state)).permissions.has(permission.id);
					if (existing) return;
					yield* Ref.update(state, (current) => ({
						...current,
						permissions: new Map(current.permissions).set(permission.id, permission),
					}));
					const command =
						typeof permission.metadata?.command === "string"
							? permission.metadata.command
							: undefined;
					const cwd =
						typeof permission.metadata?.cwd === "string"
							? permission.metadata.cwd
							: undefined;
					yield* Emit({
						...interaction_base(
							input.artisan_run_id,
							native_thread_id,
							permission.id,
							"permission.asked",
						),
						_tag: "approval",
						approval_id: permission.id,
						description: `OpenCode requests ${permission.action} for ${permission.resources.join(", ") || "the current task"}.`,
						request:
							permission.action === "shell"
								? {
										...(command === undefined ? {} : { command }),
										...(cwd === undefined ? {} : { cwd }),
										kind: "command",
									}
								: { kind: permission.action === "edit" ? "file_change" : "action" },
						state: "requested",
					});
				});

			const EmitForm = (form: OpenCode2PendingForm) =>
				Effect.gen(function* () {
					const existing = (yield* Ref.get(state)).forms.has(form.id);
					if (existing) return;
					yield* Ref.update(state, (current) => ({
						...current,
						forms: new Map(current.forms).set(form.id, form),
					}));
					for (const field of form.fields) {
						if (typeof field.key !== "string" || field.type === "external") continue;
						const options = option_records(field.options).flatMap((option) =>
							typeof option.label !== "string"
								? []
								: [
										{
											...(typeof option.description === "string"
												? { description: option.description }
												: {}),
											label: option.label,
										},
									],
						);
						yield* Emit({
							...interaction_base(
								input.artisan_run_id,
								native_thread_id,
								form.id,
								"form.created",
							),
							_tag: "question",
							header: form.title,
							multi_select: field.type === "multiselect",
							...(options.length === 0 ? {} : { options }),
							question_id: `${form.id}:${field.key}`,
							state: "requested",
							text:
								typeof field.title === "string"
									? field.title
									: typeof field.description === "string"
										? field.description
										: field.key,
						});
					}
				});

			const ReconcileInteractions = Effect.gen(function* () {
				const [permissions, forms] = yield* Effect.all(
					[
						MapApi(client.ListPermissions(native_thread_id)),
						MapApi(client.ListForms(native_thread_id)),
					],
					{ concurrency: 2 },
				);
				for (const value of permissions) {
					const permission = DecodeOpenCode2PendingPermission(value);
					if (permission !== undefined) yield* EmitPermission(permission);
				}
				for (const value of forms) {
					const form = DecodeOpenCode2PendingForm(value);
					if (form !== undefined) yield* EmitForm(form);
				}
			});

			const global_ready = yield* Deferred.make<void>();
			const PumpGlobal = Effect.gen(function* () {
				for (;;) {
					yield* client.GlobalEvents().pipe(
						Stream.runForEach((event) =>
							Effect.gen(function* () {
								const envelope =
									typeof event === "object" &&
									event !== null &&
									!Array.isArray(event)
										? (event as Readonly<Record<string, unknown>>)
										: undefined;
								const data =
									typeof envelope?.data === "object" &&
									envelope.data !== null &&
									!Array.isArray(envelope.data)
										? (envelope.data as Readonly<Record<string, unknown>>)
										: undefined;
								if (envelope?.type === "server.connected") {
									yield* Deferred.succeed(global_ready, undefined);
									return;
								}
								if (OpenCode2EventSessionId(event) !== native_thread_id) return;
								if (envelope?.type === "permission.asked") {
									const permission = DecodeOpenCode2PendingPermission(data);
									if (permission !== undefined) yield* EmitPermission(permission);
									return;
								}
								if (envelope?.type === "form.created") {
									const form = DecodeOpenCode2PendingForm(data?.form);
									if (form !== undefined) yield* EmitForm(form);
									return;
								}
								yield* IsOpenCode2DurableEvent(event)
									? ProcessDurableEvent(event)
									: ProcessEvent(event);
							}),
						),
						Effect.result,
					);
					if (yield* event_buffer.IsClosed) return;
					const cursor = yield* Ref.get(durable_cursor);
					yield* client
						.SessionLog(native_thread_id, cursor, false)
						.pipe(Stream.runForEach(ProcessDurableEvent), Effect.ignore);
					yield* ReconcileInteractions.pipe(Effect.ignore);
					yield* Effect.sleep("250 millis");
				}
			});

			yield* PumpGlobal.pipe(Effect.forkScoped, Scope.provide(run_scope));
			yield* Deferred.await(global_ready).pipe(
				Effect.timeoutOrElse({
					duration: "10 seconds",
					orElse: () =>
						Effect.fail(
							new EngineUnavailableError({
								engine_id: "opencode2",
								message: "OpenCode did not establish its private event stream.",
							}),
						),
				}),
			);
			const replay_cursor = yield* Ref.get(durable_cursor);
			yield* client.SessionLog(native_thread_id, replay_cursor, false).pipe(
				Stream.runForEach(ProcessDurableEvent),
				Effect.mapError(api_failure),
				Effect.timeoutOrElse({
					duration: "10 seconds",
					orElse: () =>
						Effect.fail(
							new EngineUnavailableError({
								engine_id: "opencode2",
								message: "OpenCode did not complete its durable replay boundary.",
							}),
						),
				}),
			);
			yield* ReconcileInteractions.pipe(Effect.ignore);
			yield* Emit({
				...interaction_base(input.artisan_run_id, native_thread_id, "opening", "run.state"),
				_tag: "run_state",
				state: "opening",
			});

			const initial_text =
				input._tag === "start"
					? input.initial_text
					: (input.next_text ??
						"Continue after Artisan reconnected to this OpenCode session.");
			const initial_content =
				input._tag === "start" ? input.initial_content : input.next_content;
			const initial_prompt_id = deterministic_id(
				"msg",
				`${input.artisan_run_id}:${
					input._tag === "start"
						? "initial"
						: input.next_text === undefined
							? "recovery"
							: "resume"
				}`,
			);
			yield* MapApi(
				client.Prompt(native_thread_id, {
					...prompt_parts(initial_text, initial_content),
					id: initial_prompt_id,
					...(input._tag === "resume" && input.next_text === undefined
						? { resume: true }
						: {}),
				}),
			);
			yield* Scope.addFinalizer(run_scope, Finish("closed"));
			const ReconcileProjection = Effect.gen(function* () {
				for (let attempt = 0; attempt < 20; attempt += 1) {
					if (yield* event_buffer.IsClosed) return;
					const result = yield* MapApi(
						client
							.Wait(native_thread_id)
							.pipe(Effect.andThen(client.ListMessages(native_thread_id))),
					).pipe(Effect.result);
					if (result._tag === "Failure") {
						yield* Effect.sleep("250 millis");
						continue;
					}
					const recovered = RecoverOpenCode2Projection(
						result.success,
						initial_prompt_id,
						native_thread_id,
					);
					const cancelled = (yield* Ref.get(state)).cancel_requested;
					if (recovered !== undefined) {
						for (const event of recovered.events) {
							const type =
								typeof event === "object" && event !== null && "type" in event
									? event.type
									: undefined;
							if (
								cancelled &&
								typeof type === "string" &&
								type.startsWith("session.execution.")
							)
								continue;
							yield* ProcessEvent(event);
						}
						if (cancelled) yield* Finish("cancelled");
						if (recovered.terminal !== undefined || cancelled) return;
					} else if (cancelled) {
						yield* Finish("cancelled");
						return;
					}
					yield* Effect.sleep("250 millis");
				}
				if (!(yield* event_buffer.IsClosed)) yield* Finish("failed", observation_failure);
			});
			yield* ReconcileProjection.pipe(Effect.forkScoped, Scope.provide(run_scope));

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
								yield* MapApi(client.Interrupt(native_thread_id));
								yield* Ref.update(state, (next) => ({
									...next,
									cancel_requested: true,
								}));
								break;
							case "close":
								yield* RememberCommand(command.command_id, intent);
								yield* Finish("closed");
								return;
							case "steer":
								yield* MapApi(
									client.Prompt(native_thread_id, {
										...prompt_parts(command.text, command.content),
										delivery: "steer",
										id: deterministic_id("msg", command.command_id),
									}),
								);
								break;
							case "respond_approval": {
								const permission = current.permissions.get(command.approval_id);
								if (permission === undefined)
									return yield* new EngineCommandTargetError({
										artisan_run_id: input.artisan_run_id,
										command_id: command.command_id,
										target: "approval",
										target_id: command.approval_id,
									});
								yield* MapApi(
									client.ReplyPermission(
										native_thread_id,
										permission.id,
										command.approved,
									),
								);
								yield* Ref.update(state, (next) => {
									const permissions = new Map(next.permissions);
									permissions.delete(permission.id);
									return { ...next, permissions };
								});
								yield* Emit({
									...interaction_base(
										input.artisan_run_id,
										native_thread_id,
										permission.id,
										"permission.replied",
									),
									_tag: "approval",
									approval_id: permission.id,
									approved: command.approved,
									description: `OpenCode ${permission.action} request resolved.`,
									request: { kind: "action" },
									state: "resolved",
								});
								break;
							}
							case "respond_question": {
								const form_ids = new Set(
									Object.keys(command.answers).flatMap((question_id) => {
										const form_id = question_id.split(":", 1)[0];
										return form_id === undefined ? [] : [form_id];
									}),
								);
								for (const form_id of form_ids) {
									const form = current.forms.get(form_id);
									if (form === undefined)
										return yield* new EngineCommandTargetError({
											artisan_run_id: input.artisan_run_id,
											command_id: command.command_id,
											target: "question",
											target_id: form_id,
										});
									yield* MapApi(
										client.ReplyForm(
											native_thread_id,
											form.id,
											OpenCode2FormAnswer(form, command.answers),
										),
									);
									yield* Ref.update(state, (next) => {
										const forms = new Map(next.forms);
										forms.delete(form.id);
										return { ...next, forms };
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
					opaque_checkpoint: "opencode2:durable-log-v1",
				},
				Send,
			};
		});

	return {
		Catalog,
		CheckNativeContinuation: (input) =>
			Effect.succeed(
				input.target_model === undefined || input.target_model.trim().length > 0
					? ({ state: "compatible" } as const)
					: ({
							reason: "The requested OpenCode model is empty.",
							state: "incompatible",
						} as const),
			),
		Descriptor: OpenCode2EngineDescriptor,
		Connections,
		Open,
		Probe,
		Usage: Connections.List({
			profile_id: "default",
			working_directory: process.cwd(),
			workspace_trust: "safe",
		}).pipe(
			Effect.map((connections) => ({
				authentication: {
					state: connections.some(
						(connection) => connection.id === "opencode" && connection.connected,
					)
						? ("authenticated" as const)
						: ("unauthenticated" as const),
				},
				windows: [],
			})),
		),
	};
};

export const make_opencode2_engine_layer = (
	options: OpenCode2EngineOptions = {},
): Layer.Layer<OpenCode2Engine, never, EngineProcessFactory> =>
	Layer.effect(
		OpenCode2Engine,
		Effect.gen(function* () {
			const base_factory = yield* EngineProcessFactory;
			const factory =
				options.ResolveSpawnOverride === undefined
					? base_factory
					: with_engine_spawn_override(base_factory, options.ResolveSpawnOverride);
			const service_scope = yield* Scope.Scope;
			const pool_lock = yield* Semaphore.make(1);
			const services = yield* Ref.make<ReadonlyMap<string, OpenCode2PrivateService>>(
				new Map(),
			);
			const PooledStart: NonNullable<OpenCode2EngineOptions["StartService"]> = (input) =>
				Semaphore.withPermit(pool_lock)(
					Effect.gen(function* () {
						const existing = (yield* Ref.get(services)).get(input.profile_id);
						if (existing !== undefined) {
							const health = yield* existing.Client.Health.pipe(Effect.result);
							if (health._tag === "Success")
								return { ...existing, Close: Effect.void };
							yield* existing.Close;
							yield* Ref.update(services, (current) => {
								const next = new Map(current);
								next.delete(input.profile_id);
								return next;
							});
						}
						const service = yield* StartOpenCode2PrivateService({
							...input,
							factory,
						}).pipe(Scope.provide(service_scope));
						yield* Ref.update(services, (current) =>
							new Map(current).set(input.profile_id, service),
						);
						return { ...service, Close: Effect.void };
					}),
				);
			return MakeOpenCode2Engine(factory, options.StartService ?? PooledStart);
		}),
	);
