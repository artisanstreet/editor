import { describe, expect, it } from "@effect/vitest";
import { Effect, Layer, Schema } from "effect";

import { model_manifest } from "@artisan/catalog";
import { make_engine_registry_layer, type Engine } from "@artisan/engines";
import { ThreadSessionPolicy, type ThreadSessionPolicy as Policy } from "@artisan/protocol";
import {
	RuntimeCatalogLive,
	RuntimeCatalogService,
} from "../../modules/backend/src/runtime/runtime-catalog";

/** The catalog consumes only the descriptor identity of a registered engine. */
const registered_engine = (id: string) => ({ Descriptor: { id } }) as unknown as Engine;

const catalog_layer = RuntimeCatalogLive.pipe(
	Layer.provideMerge(
		make_engine_registry_layer([registered_engine("codex"), registered_engine("claude")]),
	),
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
	it.effect("accepts the supervised policy for every registered engine", () =>
		Effect.gen(function* () {
			const catalog = yield* RuntimeCatalogService;
			for (const harness of ["codex", "claude"]) {
				yield* catalog.ValidateThreadSessionPolicy(policy_for(harness));
			}
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
