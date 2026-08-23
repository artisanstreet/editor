import {
	model_manifest,
	type ModelDefinition,
	type ModelManifest,
	type ProviderDefinition,
} from "@artisan/catalog";
import {
	EngineRegistry,
	type Engine,
	type EngineCatalogModel,
	type EngineCatalogScope,
	type EngineModelCatalogSnapshot,
} from "@artisan/engines";
import {
	RuntimeCatalog,
	SessionPolicyPermission,
	type RuntimeCatalogScope,
	type ThreadSessionPolicy,
} from "@artisan/protocol";
import { Context, Data, Effect, Layer, Schema } from "effect";

export class RuntimeCatalogPolicyError extends Data.TaggedError("RuntimeCatalogPolicyError")<{
	readonly field:
		| "catalog_revision"
		| "context_window"
		| "engine_id"
		| "model"
		| "model_id"
		| "permission"
		| "profile_id"
		| "provider_route_id"
		| "reasoning_effort"
		| "service_tier"
		| "variant_id";
	readonly message: string;
}> {}

/** Owns curated capabilities plus location/profile-scoped live engine inventory. */
export class RuntimeCatalogService extends Context.Service<
	RuntimeCatalogService,
	{
		/** Backwards-compatible default-profile/default-location catalog read. */
		readonly Get: Effect.Effect<RuntimeCatalog, RuntimeCatalogPolicyError>;
		readonly GetScoped: (
			scope?: Partial<RuntimeCatalogScope>,
		) => Effect.Effect<RuntimeCatalog, RuntimeCatalogPolicyError>;
		readonly ValidateThreadSessionPolicy: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<void, RuntimeCatalogPolicyError>;
	}
>()("Artisan/RuntimeCatalogService") {}

const default_scope = (scope: Partial<RuntimeCatalogScope> = {}): EngineCatalogScope => ({
	profile_id: scope.profile_id ?? "default",
	working_directory: scope.working_directory ?? process.cwd(),
	workspace_trust: scope.workspace_trust ?? "safe",
});

const unqualified_model_id = (model_id: string) => model_id.split("/").at(-1) ?? model_id;

/**
 * Gateways report executable route identities, not presentation metadata. When
 * their native model id names a model Artisan already knows, prefer that
 * canonical model's provider copy and configurable capability vocabulary.
 */
const canonical_model_for = (model: EngineCatalogModel): ModelDefinition | undefined => {
	const native_ids = new Set(
		[model.model_id, model.upstream_model_id]
			.filter((id): id is string => id !== undefined)
			.flatMap((id) => [id, unqualified_model_id(id)]),
	);
	const exact_candidates = model_manifest.models.filter((candidate) =>
		native_ids.has(candidate.native_model_id),
	);
	/** Hermes adds `-pro` to some provider routes without changing the base model. */
	const alias_ids = new Set(
		[...native_ids].flatMap((id) => (id.endsWith("-pro") ? [id.slice(0, -4)] : [])),
	);
	const candidates =
		exact_candidates.length > 0
			? exact_candidates
			: model_manifest.models.filter((candidate) => alias_ids.has(candidate.native_model_id));
	const provider_hint = model.model_id.includes("/") ? model.model_id.split("/")[0] : undefined;
	return (
		candidates.find(
			(candidate) => candidate.provider === provider_hint && candidate.harness !== "cursor",
		) ??
		candidates.find((candidate) => candidate.provider === provider_hint) ??
		candidates.find((candidate) => candidate.harness !== "cursor") ??
		candidates[0]
	);
};

const reported_provider_namespaces: Readonly<Record<string, string>> = {
	anthropic: "anthropic",
	deepseek: "deepseek",
	google: "google",
	meta: "meta",
	minimax: "minimax",
	moonshot: "moonshot",
	moonshotai: "moonshot",
	nvidia: "nvidia",
	openai: "openai",
	qwen: "qwen",
	tencent: "tencent",
	xai: "xai",
	xiaomi: "xiaomi",
	zhipu: "zai",
	zhipuai: "zai",
};

const reported_provider_patterns: ReadonlyArray<readonly [RegExp, string]> = [
	[/^claude(?:[-_.]|$)/u, "anthropic"],
	[/^(?:chatgpt|codex|gpt|o[1345])(?:[-_.]|$)/u, "openai"],
	[/^(?:gemini|gemma)(?:[-_.]|$)/u, "google"],
	[/^grok(?:[-_.]|$)/u, "xai"],
	[/^nemotron(?:[-_.]|$)/u, "nvidia"],
	[/^muse(?:[-_.]|$)/u, "meta"],
	[/^hy3(?:[-_.]|$)/u, "tencent"],
	[/^mimo(?:[-_.]|$)/u, "xiaomi"],
	[/^minimax(?:[-_.]|$)/u, "minimax"],
	[/^glm(?:[-_.]|$)/u, "zai"],
	[/^kimi(?:[-_.]|$)/u, "moonshot"],
	[/^qwen(?:[-_.]|$)/u, "qwen"],
	[/^deepseek(?:[-_.]|$)/u, "deepseek"],
];

/** Resolves the lab from provider-qualified or stable family model IDs. */
const reported_provider_for = (model: EngineCatalogModel): string | undefined => {
	for (const reported_id of [model.upstream_model_id, model.model_id]) {
		if (reported_id === undefined) continue;
		const normalized = reported_id.toLowerCase();
		const [namespace] = normalized.split("/");
		const namespaced_provider =
			normalized.includes("/") && namespace !== undefined
				? reported_provider_namespaces[namespace]
				: undefined;
		if (namespaced_provider !== undefined) return namespaced_provider;
		const model_id = unqualified_model_id(normalized);
		const matched = reported_provider_patterns.find(([pattern]) => pattern.test(model_id));
		if (matched !== undefined) return matched[1];
	}
	return undefined;
};

const opencode_descriptions: ReadonlyArray<readonly [RegExp, string]> = [
	[
		/^muse-spark(?:[-_.].*)?$/iu,
		"Meta is back. A nimble and steerable coder that stays fast across huge context and keeps going when others stall.",
	],
	[
		/^ox-alpha(?:[-_.].*)?$/iu,
		"A dark horse. Lean and mysterious, unusually good at agentic tool use. Small footprint with outsized results on messy repos.",
	],
	[
		/^nemotron(?:[-_.].*)?$/iu,
		"Built for speed without dulling the edge. Instant answers with long coherence, great for rapid iteration.",
	],
	[
		/^hy3(?:[-_.].*)?$/iu,
		"A hybrid thinker that balances chat and code with low latency. Clean instruction following and tidy diffs.",
	],
	[
		/^deepseek(?:[-_.].*)?$/iu,
		"Reasoning first, deliberate and thorough. Strong on hard coding puzzles.",
	],
	[
		/^qwen(?:[-_.].*)?$/iu,
		"A versatile all rounder with solid code and chat, plus a big window to keep everything in sight.",
	],
	[
		/^kimi(?:[-_.].*)?$/iu,
		"A long context storyteller that holds the thread. Great for big refactors.",
	],
	[
		/^minimax(?:[-_.].*)?$/iu,
		"Creative and fast, with a knack for large edits without losing style.",
	],
	[
		/^glm(?:[-_.].*)?$/iu,
		"Balanced reasoning and code, crisp on instructions.",
	],
	[
		/^mimo(?:[-_.].*)?$/iu,
		"Lightweight and quick, handy for sweeps and boilerplate.",
	],
];

const opencode_description_for = (model: EngineCatalogModel): string | undefined => {
	const raw = unqualified_model_id(model.model_id).toLowerCase();
	for (const [pattern, description] of opencode_descriptions) {
		if (pattern.test(raw)) return description;
	}
	return undefined;
};

const provider_defined_speed = (
	snapshot: EngineModelCatalogSnapshot,
	hermes: boolean,
): ModelDefinition["capabilities"]["speed_options"][number] => ({
	availability: "always",
	consumption_basis: "standard",
	consumption_multiplier: null,
	default: true,
	description: hermes
		? "Provider-defined delivery through the selected Hermes provider."
		: "Provider-defined delivery through the selected OpenCode route.",
	id: "standard",
	label: "Standard",
	native_value: "standard",
	source_url: hermes
		? "https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models"
		: "https://opencode.ai/v2/docs/models/",
	speed_multiplier: null,
	verified_at: snapshot.generated_at,
});

const hermes_fast_speed = (
	snapshot: EngineModelCatalogSnapshot,
): ModelDefinition["capabilities"]["speed_options"][number] => ({
	availability: "dynamic",
	consumption_basis: "standard",
	consumption_multiplier: null,
	default: false,
	description:
		"Requests Hermes fast delivery for this model. Availability and billing remain provider-defined.",
	id: "fast",
	label: "Fast",
	native_value: "fast",
	source_url: "https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models",
	speed_multiplier: null,
	verified_at: snapshot.generated_at,
});

const dynamic_speed_options = (
	snapshot: EngineModelCatalogSnapshot,
	model: EngineCatalogModel,
	hermes: boolean,
	canonical: ModelDefinition | undefined,
): ModelDefinition["capabilities"]["speed_options"] => {
	const standard =
		canonical?.capabilities.speed_options.find(
			(option) => option.native_value === "standard",
		) ?? provider_defined_speed(snapshot, hermes);
	if (!hermes || model.capabilities.fast !== true) return [{ ...standard, default: true }];
	const fast =
		canonical?.capabilities.speed_options.find((option) => option.native_value === "fast") ??
		hermes_fast_speed(snapshot);
	return [
		{ ...standard, default: true },
		{ ...fast, default: false },
	];
};

const dynamic_thinking = (
	hermes: boolean,
	model: EngineCatalogModel,
	canonical: ModelDefinition | undefined,
): ModelDefinition["capabilities"]["thinking"] => {
	if (!hermes)
		return {
			availability: "native",
			description:
				"OpenCode model variants carry provider-owned reasoning settings separately.",
		};
	if (model.capabilities.reasoning === false) return { availability: "unavailable" };
	const canonical_thinking = canonical?.capabilities.thinking;
	if (model.capabilities.reasoning === true && canonical_thinking?.availability === "supported") {
		/** Ultra is Codex subagent orchestration, not a model effort Hermes can forward. */
		const [first, ...rest] = canonical_thinking.options.filter(
			(option) => option.economics !== "harness-orchestration",
		);
		if (first !== undefined) {
			const options = [first, ...rest] as const;
			return {
				availability: "supported",
				default: options.some((option) => option.id === canonical_thinking.default)
					? canonical_thinking.default
					: first.id,
				options,
			};
		}
	}
	return {
		availability: "native",
		description: "Hermes owns provider-specific reasoning behavior for this model.",
	};
};

const dynamic_model_definition = (
	engine: Engine,
	snapshot: EngineModelCatalogSnapshot,
	model: EngineCatalogModel,
): ModelDefinition => {
	const hermes = engine.Descriptor.id === "hermes";
	const canonical = canonical_model_for(model);
	const managed_opencode_route =
		engine.Descriptor.id === "opencode2" &&
		(model.provider_route_id === "opencode" || model.provider_route_id === "opencode-go");
	const provider =
		canonical?.provider ??
		reported_provider_for(model) ??
		(managed_opencode_route ? "unknown" : model.provider_route_id);
	const opencode_fallback_description =
		canonical?.description === undefined && !hermes
			? opencode_description_for(model)
			: undefined;
	const description = canonical?.description ?? opencode_fallback_description;
	return {
		capabilities: {
			...(model.capabilities.context_window_tokens === undefined
				? {}
				: { context_window_tokens: model.capabilities.context_window_tokens }),
			image_input: model.capabilities.image_input,
			local_tools: model.capabilities.tools,
			mcp: model.capabilities.tools,
			...(model.capabilities.output_tokens === undefined
				? {}
				: { output_tokens: model.capabilities.output_tokens }),
			...(canonical?.capabilities.reasoning_display === undefined
				? {}
				: { reasoning_display: canonical.capabilities.reasoning_display }),
			speed_options: dynamic_speed_options(snapshot, model, hermes, canonical),
			thinking: dynamic_thinking(hermes, model, canonical),
			web_search: model.capabilities.tools,
		},
		...(model.cost === undefined ? {} : { cost: model.cost }),
		...(description === undefined ? {} : { description }),
		...(model.enabled
			? {}
			: {
					disabled: {
						reason: hermes
							? "The selected Hermes provider is unavailable or does not permit this model."
							: "The live OpenCode provider catalog reports this model disabled.",
					},
				}),
		harness: engine.Descriptor.id as ModelDefinition["harness"],
		id: model.catalog_id,
		metadata_confidence: model.metadata_confidence,
		/** Live gateways own the executable id; Artisan owns presentation for known models. */
		name: canonical?.name ?? model.name,
		native_model_id: model.model_id,
		native_selection: {
			model_id: model.model_id,
			provider_route_id: model.provider_route_id,
			...(model.variant_id === undefined ? {} : { variant_id: model.variant_id }),
		},
		provider,
		routing: { kind: "provider-route", provider_route_id: model.provider_route_id },
		status: "dynamic",
		...(model.upstream_model_id === undefined
			? {}
			: { upstream_model_id: model.upstream_model_id }),
	};
};

const merge_live_inventory = (
	engines: ReadonlyArray<Engine>,
	snapshots: ReadonlyArray<EngineModelCatalogSnapshot>,
) => {
	const engine_by_id = new Map(engines.map((engine) => [engine.Descriptor.id, engine]));
	const live_models = snapshots.flatMap((snapshot) => {
		const engine = engine_by_id.get(snapshot.engine_id);
		return engine === undefined
			? []
			: snapshot.models.map((model) => dynamic_model_definition(engine, snapshot, model));
	});
	const providers = new Map<string, ProviderDefinition>(
		model_manifest.providers.map((provider) => [provider.id, provider]),
	);
	for (const snapshot of snapshots) {
		for (const route of snapshot.routes) {
			if (!providers.has(route.id)) {
				providers.set(route.id, {
					id: route.id,
					label: route.label,
				});
			}
		}
	}
	const revision = [
		model_manifest.revision,
		...snapshots.map((snapshot) => `${snapshot.engine_id}:${snapshot.revision}`),
	].join("+");
	const manifest: ModelManifest = {
		harnesses: model_manifest.harnesses,
		models: [...model_manifest.models, ...live_models],
		providers: [...providers.values()],
		revision,
	};
	const routes = snapshots.flatMap((snapshot) =>
		snapshot.routes.map((route) => ({
			...route,
			engine_id: snapshot.engine_id,
		})),
	);
	return { manifest, routes };
};

export const RuntimeCatalogLive = Layer.effect(
	RuntimeCatalogService,
	Effect.gen(function* () {
		const registry = yield* EngineRegistry;
		const engines = yield* registry.List;
		const runnable = new Set(engines.map((engine) => engine.Descriptor.id));
		const default_model_id = model_manifest.models.find(
			(model) => model.disabled === undefined && runnable.has(model.harness),
		)?.id;

		const GetScoped = (requested_scope: Partial<RuntimeCatalogScope> = {}) =>
			Effect.gen(function* () {
				const scope = default_scope(requested_scope);
				const snapshots = (yield* Effect.forEach(
					engines,
					(engine) =>
						engine.Catalog === undefined
							? Effect.succeed(undefined)
							: engine.Catalog(scope).pipe(Effect.orElseSucceed(() => undefined)),
					{ concurrency: "unbounded" },
				)).filter(
					(snapshot): snapshot is EngineModelCatalogSnapshot => snapshot !== undefined,
				);
				const { manifest, routes } = merge_live_inventory(engines, snapshots);
				return yield* Schema.decodeUnknownEffect(RuntimeCatalog)({
					catalog_revision: manifest.revision,
					...(default_model_id === undefined ? {} : { default_model_id }),
					manifest,
					runnable_harness_ids: [...runnable],
					routes,
					...(snapshots.length === 0 ? {} : { scope }),
				}).pipe(
					Effect.mapError(
						() =>
							new RuntimeCatalogPolicyError({
								field: "model",
								message: "The runtime catalog manifest does not match its schema.",
							}),
					),
				);
			});

		const ValidateThreadSessionPolicy = (policy: ThreadSessionPolicy) =>
			Effect.gen(function* () {
				const harness = model_manifest.harnesses.find(
					(candidate) => candidate.id === policy.engine_id,
				);
				if (harness === undefined) {
					return yield* new RuntimeCatalogPolicyError({
						field: "engine_id",
						message: `Engine ${policy.engine_id} is not in the catalog.`,
					});
				}
				if (!runnable.has(policy.engine_id)) {
					return yield* new RuntimeCatalogPolicyError({
						field: "engine_id",
						message: `Engine ${policy.engine_id} has no registered runtime in this Forge process.`,
					});
				}

				const permission_option_id = SessionPolicyPermission(policy);
				if (
					!harness.permissions.options.some(
						(option) => option.id === permission_option_id,
					)
				) {
					return yield* new RuntimeCatalogPolicyError({
						field: "permission",
						message: `Permission ${permission_option_id} is unavailable for ${policy.engine_id}.`,
					});
				}

				if (policy.engine_id === "opencode2" || policy.engine_id === "hermes") {
					for (const [field, value] of [
						["profile_id", policy.profile_id],
						["provider_route_id", policy.provider_route_id],
						["model_id", policy.model_id],
						["catalog_revision", policy.catalog_revision],
					] as const) {
						if (value === undefined || value.trim().length === 0)
							return yield* new RuntimeCatalogPolicyError({
								field,
								message: `${policy.engine_id === "hermes" ? "Hermes" : "OpenCode"} requires ${field}.`,
							});
					}
					if (policy.model !== undefined && policy.model !== policy.model_id)
						return yield* new RuntimeCatalogPolicyError({
							field: "model",
							message: `${policy.engine_id === "hermes" ? "Hermes" : "OpenCode"} compatibility model and structured model_id disagree.`,
						});
					if (policy.context_window !== undefined)
						return yield* new RuntimeCatalogPolicyError({
							field: "context_window",
							message: `${policy.engine_id === "hermes" ? "Hermes" : "OpenCode"} context limits are reported metadata, not a selectable suffix.`,
						});
					if (
						policy.service_tier !== "standard" &&
						!(policy.engine_id === "hermes" && policy.service_tier === "fast")
					)
						return yield* new RuntimeCatalogPolicyError({
							field: "service_tier",
							message:
								policy.engine_id === "hermes"
									? `Hermes does not support service tier ${policy.service_tier}.`
									: "OpenCode routes do not map Artisan service tiers.",
						});
					return;
				}

				const catalog = yield* GetScoped();
				const default_model = catalog.manifest.models.find(
					(model) => model.id === catalog.default_model_id,
				);
				const model =
					policy.model === undefined
						? default_model
						: catalog.manifest.models.find(
								(candidate) =>
									candidate.harness === policy.engine_id &&
									candidate.native_model_id === policy.model,
							);
				if (model === undefined || model.harness !== policy.engine_id) {
					return yield* new RuntimeCatalogPolicyError({
						field: "model",
						message: `Model ${policy.model ?? "<default>"} is unavailable for ${policy.engine_id}.`,
					});
				}
				if (model.disabled !== undefined) {
					return yield* new RuntimeCatalogPolicyError({
						field: "model",
						message: `Model ${model.native_model_id} is disabled: ${model.disabled.reason}`,
					});
				}
				if (
					model.capabilities.thinking.availability === "supported" &&
					!model.capabilities.thinking.options.some(
						(option) => option.native_value === policy.reasoning_effort,
					)
				) {
					return yield* new RuntimeCatalogPolicyError({
						field: "reasoning_effort",
						message: `Reasoning effort ${policy.reasoning_effort} is unavailable for ${model.id}.`,
					});
				}
				if (policy.context_window !== undefined) {
					const context_capability = model.capabilities.context_window;
					if (
						context_capability === undefined ||
						!context_capability.options.some(
							(option) => option.native_suffix === policy.context_window,
						)
					) {
						return yield* new RuntimeCatalogPolicyError({
							field: "context_window",
							message: `Context window ${policy.context_window} is unavailable for ${model.id}.`,
						});
					}
				}
				if (
					!model.capabilities.speed_options.some(
						(option) =>
							option.native_value === policy.service_tier &&
							option.disabled === undefined,
					)
				) {
					return yield* new RuntimeCatalogPolicyError({
						field: "service_tier",
						message: `Service tier ${policy.service_tier} is unavailable for ${model.id}.`,
					});
				}
			});

		return RuntimeCatalogService.of({
			Get: GetScoped(),
			GetScoped,
			ValidateThreadSessionPolicy,
		});
	}),
);
