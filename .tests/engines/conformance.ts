import { Effect, Layer } from "effect";

import type {
	CodexProcessFactory,
	Engine,
	EngineInspectInput,
	EngineInspection,
} from "@artisan/engines";

/** Defines a portable inspection conformance case for any engine adapter. @since 0.1.0 */
export interface EngineInspectionScenario {
	readonly engine: Engine<CodexProcessFactory>;
	readonly input: EngineInspectInput;
	readonly name: string;
	readonly verify: (inspection: EngineInspection) => void;
}

/**
 * Runs one shared engine inspection conformance scenario against a process layer.
 *
 * @since 0.1.0
 * @param scenario - The engine input and result assertion to execute.
 * @param process_layer - The process factory implementation used by the scenario.
 * @returns A promise that completes once the scenario assertion passes.
 */
export function run_engine_inspection_scenario(
	scenario: EngineInspectionScenario,
	process_layer: Layer.Layer<CodexProcessFactory>,
): Promise<void> {
	return Effect.runPromise(
		scenario.engine.Inspect(scenario.input).pipe(
			Effect.tap((inspection) => Effect.sync(() => scenario.verify(inspection))),
			Effect.provide(process_layer),
		),
	).then(() => undefined);
}
