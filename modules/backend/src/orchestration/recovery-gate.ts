import { Cause, Context, Deferred, Effect, Exit, Layer } from "effect";

import { AgentGraphOrchestrator } from "./agent-graph-orchestrator";
import { AgentOrchestrator } from "./agent-orchestrator";

/**
 * Holds the single durable startup-recovery result for this runtime scope.
 *
 * Commands wait on this barrier so they cannot observe a stale root ownership
 * record or a graph that has not yet terminalized stranded child runs.
 */
export class OrchestrationRecoveryGate extends Context.Service<
	OrchestrationRecoveryGate,
	{
		readonly Await: Effect.Effect<void, unknown>;
		readonly Complete: (exit: Exit.Exit<void, unknown>) => Effect.Effect<boolean>;
	}
>()("Artisan/OrchestrationRecoveryGate") {}

export const OrchestrationRecoveryGateLive = Layer.effect(
	OrchestrationRecoveryGate,
	Effect.gen(function* () {
		const result = yield* Deferred.make<Exit.Exit<void, unknown>>();

		return {
			Await: Deferred.await(result).pipe(
				Effect.flatMap((exit) =>
					Exit.isSuccess(exit) ? Effect.void : Effect.failCause(exit.cause),
				),
			),
			Complete: (exit) => Deferred.succeed(result, exit),
		};
	}),
);

/** Starts ordered root then graph recovery without delaying layer construction. */
export class OrchestrationRecoveryCoordinator extends Context.Service<
	OrchestrationRecoveryCoordinator,
	Record<string, never>
>()("Artisan/OrchestrationRecoveryCoordinator") {}

export const OrchestrationRecoveryCoordinatorLive = Layer.effect(
	OrchestrationRecoveryCoordinator,
	Effect.gen(function* () {
		const gate = yield* OrchestrationRecoveryGate;
		const root = yield* AgentOrchestrator;
		const graph = yield* AgentGraphOrchestrator;

		/**
		 * `exit` turns both typed failure and interruption into the shared value,
		 * so every waiter settles even while the host scope is closing.
		 */
		yield* Effect.forkScoped(
			Effect.gen(function* () {
				const recovery = yield* root.Recover.pipe(
					Effect.andThen(graph.Recover),
					Effect.exit,
				);

				yield* gate.Complete(recovery);
				if (Exit.isSuccess(recovery)) {
					yield* root.StartDispatching;
					yield* graph.StartDispatching;
				} else if (!Cause.hasInterruptsOnly(recovery.cause)) {
					yield* Effect.logError(
						"Orchestration startup recovery failed; interrupted work stays unavailable until restart",
						{ cause: recovery.cause },
					);
				}
			}).pipe(
				Effect.onExit((exit) =>
					Exit.isFailure(exit)
						? gate.Complete(exit).pipe(Effect.asVoid, Effect.uninterruptible)
						: Effect.void,
				),
			),
		);

		return {};
	}),
);
