import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import {
	type Engine,
	EngineRegistry,
	EngineUnsupportedOperationError,
	make_engine_registry_layer,
} from "@artisan/engines";

function make_engine(id: string): Engine {
	const unsupported = (operation: EngineUnsupportedOperationError["operation"]) =>
		Effect.fail(new EngineUnsupportedOperationError({ engine_id: id, operation }));

	return {
		Approve: () => unsupported("approval"),
		Cancel: () => unsupported("cancel"),
		Close: () => unsupported("close"),
		Descriptor: {
			capabilities: {
				approval: "unsupported",
				cancel: "unsupported",
				close: "unsupported",
				inspect: "unsupported",
				resume: "unsupported",
				start: "unsupported",
				steer: "unsupported",
			},
			display_name: id,
			id,
			transport: "test",
		},
		Inspect: () => unsupported("inspect"),
		Resume: () => unsupported("resume"),
		Start: () => unsupported("start"),
		Steer: () => unsupported("steer"),
	};
}

describe("engine registry", () => {
	it("lists and looks up explicitly registered engines", async () => {
		const first = make_engine("first");
		const second = make_engine("second");
		const layer = make_engine_registry_layer([first, second]);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* EngineRegistry;

				return {
					found: yield* registry.Get("second"),
					listed: yield* registry.List,
				};
			}).pipe(Effect.provide(layer)),
		);

		expect(result.listed.map((engine) => engine.Descriptor.id)).toEqual(["first", "second"]);
		expect(result.found.Descriptor).toBe(second.Descriptor);
	});

	it("rejects duplicate identifiers and unknown lookups", async () => {
		const duplicate_layer = make_engine_registry_layer([
			make_engine("same"),
			make_engine("same"),
		]);
		const unique_layer = make_engine_registry_layer([make_engine("known")]);

		await expect(
			Effect.runPromise(Effect.succeed("ready").pipe(Effect.provide(duplicate_layer))),
		).rejects.toMatchObject({
			_tag: "EngineRegistryError",
			reason: "duplicate_id",
		});
		await expect(
			Effect.runPromise(
				Effect.gen(function* () {
					const registry = yield* EngineRegistry;

					return yield* registry.Get("unknown");
				}).pipe(Effect.provide(unique_layer)),
			),
		).rejects.toMatchObject({
			_tag: "EngineRegistryError",
			reason: "not_found",
		});
	});
});
