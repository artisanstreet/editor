import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer, Schema } from "effect";

import { model_manifest } from "@artisan/catalog";
import { make_engine_registry_layer, type Engine } from "@artisan/engines";
import { ThreadSessionPolicy, type ThreadSessionPolicy as Policy } from "@artisan/protocol";
import {
	RuntimeCatalogLive,
	RuntimeCatalogService,
} from "../../modules/backend/src/runtime/catalog";

/** The catalog consumes only the descriptor identity of a registered engine. */
const registered_engine = (id: string) => ({ Descriptor: { id } }) as unknown as Engine;

const catalog_layer = RuntimeCatalogLive.pipe(
	Layer.provideMerge(
		make_engine_registry_layer([registered_engine("codex"), registered_engine("claude")]),
	),
);

const opencode2_engine = {
	Catalog: (scope: {
		readonly profile_id: string;
		readonly working_directory: string;
		readonly workspace_trust: "safe" | "trusted_project_config";
	}) =>
		Effect.succeed({
			engine_id: "opencode2",
			generated_at: "2026-08-21T00:00:00.000Z",
			models: [
				["opencode", "ox-alpha-free", "Ox Alpha Free"],
				["opencode", "nemotron-3.5-lightning-free", "Nemotron 3.5 Lightning Free"],
				["opencode", "muse-spark-1.2-contributor-free", "Muse Spark 1.2 Free"],
				["opencode", "hy3-free", "Hy3 Free"],
				["opencode-go", "claude-sonnet-4-5", "Claude Sonnet 4.5"],
			].map(([provider_route_id, model_id, name]) => ({
				capabilities: {
					context_window_tokens: 200_000,
					image_input: true,
					tools: true,
				},
				catalog_id: `opencode2:${provider_route_id}:${model_id}`,
				enabled: true,
				metadata_confidence: "reported" as const,
				model_id,
				name,
				provider_route_id,
				status: "active" as const,
			})),
			revision: "live-1",
			routes: [
				{
					group: { id: "go", label: "Go", order: 0, show_route_labels: false },
					id: "opencode-go",
					label: "Go",
					status: "available" as const,
				},
				{
					group: { id: "zen", label: "Zen", order: 1, show_route_labels: false },
					id: "opencode",
					label: "Zen",
					status: "available" as const,
				},
			],
			scope,
		}),
	Descriptor: { id: "opencode2" },
} as unknown as Engine;

const opencode2_catalog_layer = RuntimeCatalogLive.pipe(
	Layer.provideMerge(make_engine_registry_layer([opencode2_engine])),
);

const hermes_engine = {
	Catalog: (scope: {
		readonly profile_id: string;
		readonly working_directory: string;
		readonly workspace_trust: "safe" | "trusted_project_config";
	}) =>
		Effect.succeed({
			engine_id: "hermes",
			generated_at: "2026-08-21T00:00:00.000Z",
			models: ["nous", "openrouter"].map((provider_route_id) => ({
				capabilities: { image_input: false, tools: true },
				catalog_id: `hermes:${provider_route_id}:anthropic/claude-sonnet-4.6`,
				enabled: true,
				metadata_confidence: "reported" as const,
				model_id: "anthropic/claude-sonnet-4.6",
				name: "Claude Sonnet 4.6",
				provider_route_id,
				status: "active" as const,
			})),
			revision: "hermes-live-1",
			routes: [
				{
					group: { id: "nous", label: "Nous Portal", order: 0, show_route_labels: false },
					id: "nous",
					label: "Nous Portal",
					status: "available" as const,
				},
				{
					group: {
						id: "openrouter",
						label: "OpenRouter",
						order: 1,
						show_route_labels: false,
					},
					id: "openrouter",
					label: "OpenRouter",
					status: "available" as const,
				},
			],
			scope,
		}),
	Descriptor: { id: "hermes" },
} as unknown as Engine;

const hermes_catalog_layer = RuntimeCatalogLive.pipe(
	Layer.provideMerge(make_engine_registry_layer([hermes_engine])),
);

const hermes_openai_engine = {
	Catalog: (scope: {
		readonly profile_id: string;
		readonly working_directory: string;
		readonly workspace_trust: "safe" | "trusted_project_config";
	}) =>
		Effect.succeed({
			engine_id: "hermes",
			generated_at: "2026-08-21T00:00:00.000Z",
			models: [
				{
					capabilities: {
						fast: true,
						image_input: false,
						reasoning: true,
						tools: true,
					},
					catalog_id: "hermes:openai-codex:openai/gpt-5.4-mini",
					enabled: true,
					metadata_confidence: "reported" as const,
					model_id: "openai/gpt-5.4-mini",
					name: "gpt-5.4-mini",
					provider_route_id: "openai-codex",
					status: "active" as const,
					upstream_model_id: "openai/gpt-5.4-mini",
				},
				{
					capabilities: {
						fast: true,
						image_input: false,
						reasoning: true,
						tools: true,
					},
					catalog_id: "hermes:openai-codex:openai/gpt-5.6-sol-pro",
					enabled: true,
					metadata_confidence: "reported" as const,
					model_id: "openai/gpt-5.6-sol-pro",
					name: "gpt-5.6-sol-pro",
					provider_route_id: "openai-codex",
					status: "active" as const,
					upstream_model_id: "openai/gpt-5.6-sol-pro",
				},
			],
			revision: "hermes-openai-live-1",
			routes: [
				{
					group: {
						id: "openai-codex",
						label: "OpenAI Codex",
						order: 0,
						show_route_labels: false,
					},
					id: "openai-codex",
					label: "OpenAI Codex",
					status: "available" as const,
				},
			],
			scope,
		}),
	Descriptor: { id: "hermes" },
} as unknown as Engine;

const hermes_openai_catalog_layer = RuntimeCatalogLive.pipe(
	Layer.provideMerge(make_engine_registry_layer([hermes_openai_engine])),
);

const manifest_model = (harness: string) => {
	const model = model_manifest.models.find(
		(candidate) => candidate.harness === harness && candidate.disabled === undefined,
	);
	if (model === undefined) throw new Error(`No enabled ${harness} model in the manifest`);
	return model;
};

const policy_for = (harness: string, patch: Partial<Policy> = {}): Policy => {
	const model = manifest_model(harness);
	const thinking = model.capabilities.thinking;
	const reasoning_effort =
		thinking.availability === "supported"
			? (thinking.options.find((option) => option.native_value === "medium")?.native_value ??
				thinking.options[0]?.native_value)
			: "medium";
	return Schema.decodeUnknownSync(ThreadSessionPolicy)({
		engine_id: harness,
		model: model.native_model_id,
		permission_mode: "on_request",
		reasoning_effort,
		sandbox_mode: "workspace_write",
		service_tier: "standard",
		strict_clarification: false,
		web_search_enabled: false,
		...patch,
	});
};

describe("runtime catalog session policy validation", () => {
	it.effect(
		"enriches Hermes rows from the canonical model without replacing route identity",
		() =>
			Effect.gen(function* () {
				const catalog = yield* RuntimeCatalogService;
				const snapshot = yield* catalog.GetScoped({
					profile_id: "default",
					working_directory: "C:\\workspace",
					workspace_trust: "safe",
				});
				const model = snapshot.manifest.models.find(
					(candidate) => candidate.id === "hermes:openai-codex:openai/gpt-5.4-mini",
				);
				expect(model).toMatchObject({
					description: "Small, fast, and cost-efficient model for simpler coding tasks.",
					name: "GPT 5.4 Mini",
					native_model_id: "openai/gpt-5.4-mini",
					native_selection: {
						model_id: "openai/gpt-5.4-mini",
						provider_route_id: "openai-codex",
					},
					provider: "openai",
				});
				expect(model?.capabilities.thinking).toMatchObject({
					availability: "supported",
					default: "medium",
				});
				expect(
					model?.capabilities.speed_options.map((option) => option.native_value),
				).toEqual(["standard", "fast"]);
				expect(
					snapshot.manifest.providers.find((provider) => provider.id === model?.provider)
						?.label,
				).toBe("OpenAI");
				const sol_pro = snapshot.manifest.models.find(
					(candidate) => candidate.id === "hermes:openai-codex:openai/gpt-5.6-sol-pro",
				);
				expect(sol_pro).toMatchObject({
					description: "OpenAI's frontier agentic coding model.",
					name: "GPT 5.6 Sol",
					native_model_id: "openai/gpt-5.6-sol-pro",
					provider: "openai",
				});
				expect(
					sol_pro?.capabilities.thinking.availability === "supported"
						? sol_pro.capabilities.thinking.options.map((option) => option.id)
						: [],
				).toEqual(["light", "medium", "high", "xhigh", "max"]);

				yield* catalog.ValidateThreadSessionPolicy({
					catalog_revision: snapshot.catalog_revision,
					engine_id: "hermes",
					model: "openai/gpt-5.4-mini",
					model_id: "openai/gpt-5.4-mini",
					permission: "supervised",
					permission_mode: "on_request",
					profile_id: "default",
					provider_route_id: "openai-codex",
					reasoning_effort: "high",
					sandbox_mode: "workspace_write",
					service_tier: "fast",
					strict_clarification: false,
					web_search_enabled: false,
				});
			}).pipe(Effect.provide(hermes_openai_catalog_layer)),
	);

	it.effect("merges Hermes providers as distinct collapsible route groups", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			const snapshot = yield* catalog.GetScoped({
				profile_id: "default",
				working_directory: "C:\\workspace",
				workspace_trust: "safe",
			});
			expect(
				snapshot.manifest.models
					.filter((model) => model.harness === "hermes")
					.map((model) => model.native_selection?.provider_route_id),
			).toEqual(["nous", "openrouter"]);
			expect(snapshot.routes?.map((route) => route.group.label)).toEqual([
				"Nous Portal",
				"OpenRouter",
			]);
			yield* catalog.ValidateThreadSessionPolicy({
				catalog_revision: "hermes-live-1",
				engine_id: "hermes",
				model: "anthropic/claude-sonnet-4.6",
				model_id: "anthropic/claude-sonnet-4.6",
				permission: "supervised",
				permission_mode: "on_request",
				profile_id: "default",
				provider_route_id: "nous",
				reasoning_effort: "medium",
				sandbox_mode: "workspace_write",
				service_tier: "standard",
				strict_clarification: false,
				web_search_enabled: false,
			});
		}).pipe(Effect.provide(hermes_catalog_layer)),
	);

	it.effect("keeps Zen and Go as distinct route-aware live model entries", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			const snapshot = yield* catalog.GetScoped({
				profile_id: "work",
				working_directory: "C:\\workspace",
				workspace_trust: "safe",
			});
			const models = snapshot.manifest.models.filter(
				(model) => model.harness === "opencode2",
			);
			expect(models).toHaveLength(5);
			expect(models.map((model) => model.native_selection?.provider_route_id)).toEqual([
				"opencode",
				"opencode",
				"opencode",
				"opencode",
				"opencode-go",
			]);
			expect(models.every((model) => model.capabilities.context_window === undefined)).toBe(
				true,
			);
			expect(
				models.every((model) => model.capabilities.context_window_tokens === 200_000),
			).toBe(true);
			expect(snapshot.routes?.map((route) => route.group.label)).toEqual(["Go", "Zen"]);
			expect(models.map((model) => model.provider)).toEqual([
				"unknown",
				"nvidia",
				"meta",
				"tencent",
				"anthropic",
			]);
			expect(snapshot.scope).toMatchObject({ profile_id: "work" });
		}).pipe(Effect.provide(opencode2_catalog_layer)),
	);

	it.effect("requires complete structured identity for OpenCode policies", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			const incomplete = Schema.decodeUnknownSync(ThreadSessionPolicy)({
				engine_id: "opencode2",
				model: "claude-sonnet-4-5",
				permission_mode: "on_request",
				reasoning_effort: "medium",
				sandbox_mode: "workspace_write",
				service_tier: "standard",
				strict_clarification: false,
				web_search_enabled: false,
			});
			const failure = yield* catalog
				.ValidateThreadSessionPolicy(incomplete)
				.pipe(Effect.flip);
			expect(failure.field).toBe("profile_id");
			yield* catalog.ValidateThreadSessionPolicy({
				...incomplete,
				catalog_revision: "live-1",
				model_id: "claude-sonnet-4-5",
				profile_id: "work",
				provider_route_id: "opencode-go",
			});
		}).pipe(Effect.provide(opencode2_catalog_layer)),
	);

	it.effect("accepts the supervised policy for every registered engine", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			for (const harness of ["codex", "claude"]) {
				yield* catalog.ValidateThreadSessionPolicy(policy_for(harness));
			}
		}).pipe(Effect.provide(catalog_layer)),
	);

	it.effect("accepts only the special efforts declared by the selected model", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			yield* catalog.ValidateThreadSessionPolicy(
				policy_for("codex", { reasoning_effort: "ultra" }),
			);
			yield* catalog.ValidateThreadSessionPolicy(
				policy_for("claude", { reasoning_effort: "max" }),
			);
			const unsupported = yield* catalog
				.ValidateThreadSessionPolicy(policy_for("claude", { reasoning_effort: "ultra" }))
				.pipe(Effect.flip);
			expect(unsupported.field).toBe("reasoning_effort");
		}).pipe(Effect.provide(catalog_layer)),
	);

	it.effect("matches permissions by the harness-neutral option id, not Codex vocabulary", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			/** `restricted` maps to plan-only on Claude and read-only on Codex. */
			yield* catalog.ValidateThreadSessionPolicy(
				policy_for("claude", { sandbox_mode: "read_only" }),
			);
			yield* catalog.ValidateThreadSessionPolicy(
				policy_for("claude", { permission_mode: "never" }),
			);
		}).pipe(Effect.provide(catalog_layer)),
	);

	it.effect("rejects an engine that is not registered in this Forge process", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			const outcome = yield* catalog
				.ValidateThreadSessionPolicy(policy_for("grok"))
				.pipe(Effect.flip);
			expect(outcome.field).toBe("engine_id");
		}).pipe(Effect.provide(catalog_layer)),
	);

	it.effect("rejects a model that belongs to another engine", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			const outcome = yield* catalog
				.ValidateThreadSessionPolicy(
					policy_for("claude", { model: manifest_model("codex").native_model_id }),
				)
				.pipe(Effect.flip);
			expect(outcome.field).toBe("model");
		}).pipe(Effect.provide(catalog_layer)),
	);
});
