import { Effect, Layer } from "effect";
import { ThreadContinuationRepository } from "./contracts";
import { MakeLaunchStateOperations } from "./launch-state-operations";
import { MakeObservationOperations } from "./observation-operations";
import { MakePreparationOperations } from "./preparation-operations";
import { MakeReadOperations } from "./read-operations";

export * from "./contracts";

export const ThreadContinuationRepositoryLive = Layer.effect(
	ThreadContinuationRepository,
	Effect.gen(function* () {
		const reads = yield* MakeReadOperations;
		const observations = yield* MakeObservationOperations;
		const preparation = yield* MakePreparationOperations;
		const launch_state = yield* MakeLaunchStateOperations;
		return { ...reads, ...observations, ...preparation, ...launch_state };
	}),
);
