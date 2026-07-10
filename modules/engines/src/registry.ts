import { Context, Effect, Layer } from "effect";

import { type Engine, EngineRegistryError } from "./engine";

/** Provides explicit lookup and enumeration for registered engine adapters. @since 0.1.0 */
export class EngineRegistry extends Context.Service<
	EngineRegistry,
	{
		readonly Get: (engine_id: string) => Effect.Effect<Engine<any>, EngineRegistryError>;
		readonly List: Effect.Effect<ReadonlyArray<Engine<any>>>;
	}
>()("Artisan/EngineRegistry") {}

/**
 * Builds an engine registry from an explicit adapter list.
 *
 * @since 0.1.0
 * @param engines - The complete set of engine adapters to expose.
 * @returns A layer that fails when two adapters share an engine identifier.
 */
export function make_engine_registry_layer(
	engines: ReadonlyArray<Engine<any>>,
): Layer.Layer<EngineRegistry, EngineRegistryError> {
	const duplicate = engines.find(
		(engine, index) =>
			engines.findIndex(({ Descriptor }) => Descriptor.id === engine.Descriptor.id) !== index,
	);

	if (duplicate) {
		return Layer.effect(
			EngineRegistry,
			Effect.fail(
				new EngineRegistryError({
					engine_id: duplicate.Descriptor.id,
					reason: "duplicate_id",
				}),
			),
		);
	}

	const engines_by_id = new Map(engines.map((engine) => [engine.Descriptor.id, engine]));
	const Get = (engine_id: string) => {
		const engine = engines_by_id.get(engine_id);

		return engine
			? Effect.succeed(engine)
			: Effect.fail(new EngineRegistryError({ engine_id, reason: "not_found" }));
	};

	return Layer.succeed(EngineRegistry, {
		Get,
		List: Effect.succeed(engines),
	});
}
