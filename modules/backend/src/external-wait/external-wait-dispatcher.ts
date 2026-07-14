import { Context, Data, Effect, Exit, Layer, Option, Schedule, Scope, Semaphore } from "effect";

import { EngineRegistry } from "@artisan/engines";

import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import {
	ExternalWaitRepository,
	type ExternalWaitRepositoryError,
} from "./external-wait-repository";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

export interface ExternalWaitDispatchCycleResult {
	readonly materialized_outbox_ids: ReadonlyArray<string>;
	readonly released_or_skipped_outbox_ids: ReadonlyArray<string>;
}

/** Owns the scoped periodic cadence for external-wait dispatch. */
export class ExternalWaitDispatchScheduler extends Context.Service<
	ExternalWaitDispatchScheduler,
	{
		readonly Schedule: (task: Effect.Effect<void>) => Effect.Effect<never, never, Scope.Scope>;
	}
>()("Artisan/ExternalWaitDispatchScheduler") {}

/** Reports a wake-dispatch failure that must remain leased until expiry. */
export class ExternalWaitDispatcherFailure extends Data.TaggedError(
	"ExternalWaitDispatcherFailure",
)<{
	readonly cause: ExternalWaitRepositoryError | unknown;
	readonly outbox_id?: string;
}> {}

/** Claims ready wakes, materializes continuations, and alerts both work dispatchers. */
export class ExternalWaitDispatcher extends Context.Service<
	ExternalWaitDispatcher,
	{
		readonly RunOnce: Effect.Effect<
			ExternalWaitDispatchCycleResult,
			ExternalWaitDispatcherFailure
		>;
	}
>()("Artisan/ExternalWaitDispatcher") {}

/** Repeats successful wake-dispatch cycles at the production one-second cadence. */
export const ExternalWaitDispatchSchedulerLive = Layer.succeed(ExternalWaitDispatchScheduler, {
	Schedule: (task) =>
		Effect.repeat(task, Schedule.spaced("1 second")).pipe(
			Effect.delay("1 second"),
			Effect.andThen(Effect.never),
		),
});

function DispatcherFailure(cause: unknown, outbox_id?: string) {
	return new ExternalWaitDispatcherFailure({
		cause,
		...(outbox_id === undefined ? {} : { outbox_id }),
	});
}

export const ExternalWaitDispatcherLive = Layer.effect(
	ExternalWaitDispatcher,
	Effect.gen(function* () {
		const external_waits = yield* ExternalWaitRepository;
		const engines = yield* EngineRegistry;
		const orchestrator = yield* AgentOrchestrator;
		const graph_orchestrator = yield* AgentGraphOrchestrator;
		const metadata = yield* RuntimeMetadata;
		const scheduler = yield* ExternalWaitDispatchScheduler;
		const service_scope = yield* Scope.make();
		const cycle_lock = yield* Semaphore.make(1);

		const NotifyDispatchers = Effect.all([
			orchestrator.NotifyWorkAvailable,
			graph_orchestrator.NotifyWorkAvailable,
		]).pipe(Effect.asVoid);

		const DispatchWake = (outbox_id: string, now: string) =>
			Effect.gen(function* () {
				const claim = yield* external_waits.ClaimWake({
					lease_owner: metadata.instance_id,
					now,
					outbox_id,
				});

				if (Option.isNone(claim)) {
					return { _tag: "skipped" as const, outbox_id };
				}

				const engine = yield* engines
					.Get(claim.value.owner.engine_id)
					.pipe(
						Effect.catch((cause) =>
							cause.reason === "not_found"
								? Effect.succeed(undefined)
								: Effect.fail(cause),
						),
					);

				if (engine === undefined) {
					const released_at = yield* metadata.Now;

					yield* external_waits.ReleaseWake({
						lease_owner: metadata.instance_id,
						now: released_at,
						outbox_id,
					});

					return { _tag: "released" as const, outbox_id };
				}

				const materialized_at = yield* metadata.Now;

				yield* external_waits.MaterializeWake({
					lease_owner: metadata.instance_id,
					native_resume_supported:
						engine.Descriptor.capabilities.resume.state === "supported",
					now: materialized_at,
					outbox_id,
				});

				return { _tag: "materialized" as const, outbox_id };
			}).pipe(Effect.mapError((cause) => DispatcherFailure(cause, outbox_id)));

		const RunOnceUnlocked = Effect.gen(function* () {
			const now = yield* metadata.Now;
			const outbox_ids = yield* external_waits.DiscoverWakes({ now });
			const dispatched = yield* Effect.forEach(outbox_ids, (outbox_id) =>
				DispatchWake(outbox_id, now).pipe(Effect.exit),
			);
			const failed = dispatched.find(Exit.isFailure);

			if (failed) {
				return yield* Effect.failCause(failed.cause);
			}
			const completed = dispatched.filter(Exit.isSuccess).map(({ value }) => value);

			return {
				materialized_outbox_ids: completed
					.filter((result) => result._tag === "materialized")
					.map((result) => result.outbox_id),
				released_or_skipped_outbox_ids: completed
					.filter((result) => result._tag !== "materialized")
					.map((result) => result.outbox_id),
			} satisfies ExternalWaitDispatchCycleResult;
		}).pipe(
			Effect.ensuring(NotifyDispatchers),
			Effect.mapError((cause) =>
				cause instanceof ExternalWaitDispatcherFailure ? cause : DispatcherFailure(cause),
			),
		);
		const RunOnce = Semaphore.withPermit(cycle_lock)(RunOnceUnlocked);

		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.void));
		yield* RunOnce.pipe(Effect.catch(() => Effect.void));
		yield* Effect.forkIn(
			scheduler
				.Schedule(
					RunOnce.pipe(
						Effect.asVoid,
						Effect.catch(() => Effect.void),
					),
				)
				.pipe(Scope.provide(service_scope)),
			service_scope,
		);
		yield* Effect.yieldNow;

		return { RunOnce };
	}),
);
