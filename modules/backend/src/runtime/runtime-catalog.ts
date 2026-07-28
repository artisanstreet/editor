import { model_manifest } from "@artisan/catalog";
import { EngineRegistry } from "@artisan/engines";
import { RuntimeCatalog } from "@artisan/protocol";
import type { ThreadSessionPolicy } from "@artisan/protocol";
import { Context, Data, Effect, Layer, Schema } from "effect";

export class RuntimeCatalogPolicyError extends Data.TaggedError("RuntimeCatalogPolicyError")<{
	readonly field: "engine_id" | "model" | "permission" | "reasoning_effort" | "service_tier";
	readonly message: string;
}> {}

/** Owns the immutable model and harness capability snapshot for one Forge process. */
export class RuntimeCatalogService extends Context.Service<
	RuntimeCatalogService,
	{
		readonly Get: Effect.Effect<RuntimeCatalog>;
		readonly ValidateThreadSessionPolicy: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<void, RuntimeCatalogPolicyError>;
	}
>()("Artisan/RuntimeCatalogService") {}

export const RuntimeCatalogLive = Layer.effect(
	RuntimeCatalogService,
	Effect.gen(function* () {
		const registry = yield* EngineRegistry;
		const engines = yield* registry.List;
		const harness_ids = new Set(engines.map((engine) => engine.Descriptor.id));
		const harnesses = model_manifest.harnesses.filter((harness) => harness_ids.has(harness.id));
		const available_harness_ids = new Set(harnesses.map((harness) => harness.id));
		const models = model_manifest.models.filter((model) =>
			available_harness_ids.has(model.harness),
		);
		const provider_ids = new Set(models.map((model) => model.provider));
		const providers = model_manifest.providers.filter((provider) =>
			provider_ids.has(provider.id),
		);
		const default_model_id = models.find((model) => model.disabled === undefined)?.id;
		const catalog = yield* Schema.decodeUnknownEffect(RuntimeCatalog)({
			...(default_model_id === undefined ? {} : { default_model_id }),
			manifest: {
				harnesses,
				models,
				providers,
				revision: model_manifest.revision,
			},
		}).pipe(Effect.orDie);

		const ValidateThreadSessionPolicy = (policy: ThreadSessionPolicy) =>
			Effect.gen(function* () {
				const harness = catalog.manifest.harnesses.find(
					(candidate) => candidate.id === policy.engine_id,
				);
				if (harness === undefined) {
					return yield* new RuntimeCatalogPolicyError({
						field: "engine_id",
						message: `Engine ${policy.engine_id} is not registered in this Forge process.`,
					});
				}

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

				/**
				 * The provider-neutral policy resolves to a neutral permission
				 * option id; every harness names its options with the same ids
				 * while `native_value` stays provider vocabulary. Matching on
				 * the native value here would reject every non-Codex engine.
				 */
				const permission_option_id =
					policy.sandbox_mode === "read_only"
						? "restricted"
						: policy.permission_mode === "never"
							? "autonomous"
							: "supervised";
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
			Get: Effect.succeed(catalog),
			ValidateThreadSessionPolicy,
		});
	}),
);
