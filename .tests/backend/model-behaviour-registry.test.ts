import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	make_codex_auto_compaction_mapping,
	make_model_behaviour_capability_registry_layer,
	make_unsupported_auto_compaction_mapping,
	ModelBehaviourCapabilityRegistry,
} from "../../modules/backend/src/model-behaviour/model-behaviour-registry";

describe("Model Behaviour capability registry", () => {
	it("projects version-gated Codex support and truthful Claude support", async () => {
		const registry = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* ModelBehaviourCapabilityRegistry;
			}).pipe(
				Effect.provide(
					make_model_behaviour_capability_registry_layer([
						make_codex_auto_compaction_mapping({
							installed_version: "0.142.5",
							mapping_available: true,
						}),
						make_unsupported_auto_compaction_mapping(
							"claude",
							"Claude Code has no equivalent supported mapping.",
						),
					]),
				),
			),
		);

		expect(registry.Capabilities).toHaveLength(1);
		expect(registry.Capabilities[0]!.provider_support).toMatchObject([
			{
				native_key: "model_auto_compact_token_limit",
				provider_id: "codex",
				state: "supported",
			},
			{ provider_id: "claude", state: "unsupported" },
		]);
	});

	it("marks a rejected Codex mapping unavailable and leaves the native key inspectable", async () => {
		const registry = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* ModelBehaviourCapabilityRegistry;
			}).pipe(
				Effect.provide(
					make_model_behaviour_capability_registry_layer([
						make_codex_auto_compaction_mapping({
							installed_version: "0.150.0",
							mapping_available: false,
						}),
					]),
				),
			),
		);

		expect(registry.Capabilities[0]!.provider_support[0]).toMatchObject({
			native_key: "model_auto_compact_token_limit",
			state: "unavailable",
		});
	});

	it("rejects duplicate provider ownership for one canonical setting", async () => {
		const mapping = make_codex_auto_compaction_mapping({
			installed_version: "0.142.5",
			mapping_available: true,
		});
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* ModelBehaviourCapabilityRegistry;
			}).pipe(
				Effect.provide(make_model_behaviour_capability_registry_layer([mapping, mapping])),
				Effect.exit,
			),
		);

		expect(result._tag).toBe("Failure");
	});
});
