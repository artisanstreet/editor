import { Clock, Context, Data, Deferred, Effect, Exit, Layer, Ref, Scope } from "effect";

import { ThreadErasure, type ThreadErasureFailure } from "./thread-erasure";
import { ThreadRetentionPolicyService } from "./thread-retention-policy";
import type { JournalStoreError } from "../persistence/journal-store";

const day_ms = 24 * 60 * 60 * 1_000;

/** Supplies deterministic wall-clock timestamps for retention decisions. */
export class ThreadRetentionClock extends Context.Service<
	ThreadRetentionClock,
	{
		readonly Now: Effect.Effect<string>;
	}
>()("Artisan/ThreadRetentionClock") {}

/** Owns the scoped periodic execution policy for retention cleanup. */
export class ThreadRetentionScheduler extends Context.Service<
	ThreadRetentionScheduler,
	{
		readonly Schedule: (task: Effect.Effect<void>) => Effect.Effect<never, never, Scope.Scope>;
	}
>()("Artisan/ThreadRetentionScheduler") {}

/** Wraps policy, clock, or erasure failures from one cleanup cycle. */
export class ThreadRetentionFailure extends Data.TaggedError("ThreadRetentionFailure")<{
	readonly cause: JournalStoreError | ThreadErasureFailure | unknown;
}> {}

/** Runs startup, manual, and periodic inactive-thread retention cleanup. */
export class ThreadRetention extends Context.Service<
	ThreadRetention,
	{
		readonly RunCleanup: Effect.Effect<ReadonlyArray<string>, ThreadRetentionFailure>;
	}
>()("Artisan/ThreadRetention") {}

/** Uses the system UTC clock for production retention decisions. */
export const ThreadRetentionClockLive = Layer.succeed(ThreadRetentionClock, {
	Now: Clock.currentTimeMillis.pipe(
		Effect.map((milliseconds) => new Date(milliseconds).toISOString()),
	),
});

/** Runs production cleanup hourly while the backend runtime remains scoped. */
export const ThreadRetentionSchedulerLive = Layer.succeed(ThreadRetentionScheduler, {
	Schedule: (task) => Effect.forever(Effect.sleep("1 hour").pipe(Effect.andThen(task))),
});

function RetentionCutoff(now: string, inactivity_days: number) {
	const now_ms = Date.parse(now);

	if (!Number.isFinite(now_ms)) {
		return Effect.fail(
			new ThreadRetentionFailure({
				cause: new Error("Retention clock returned an invalid timestamp"),
			}),
		);
	}

	return Effect.succeed(new Date(now_ms - inactivity_days * day_ms).toISOString());
}

export const ThreadRetentionLive = Layer.effect(
	ThreadRetention,
	Effect.gen(function* () {
		const clock = yield* ThreadRetentionClock;
		const erasure = yield* ThreadErasure;
		const policy = yield* ThreadRetentionPolicyService;
		const scheduler = yield* ThreadRetentionScheduler;
		const service_scope = yield* Scope.make();
		const flight = yield* Ref.make<
			Deferred.Deferred<Exit.Exit<ReadonlyArray<string>, ThreadRetentionFailure>> | undefined
		>(undefined);
		const Cleanup = Effect.gen(function* () {
			const now = yield* clock.Now;
			const resumed = yield* erasure.ResumeClaimed(now);
			const current_policy = yield* policy.Read;

			if (!current_policy.enabled) {
				return resumed;
			}

			const cutoff = yield* RetentionCutoff(now, current_policy.inactivity_days);
			const expired = yield* erasure.CleanupExpired(cutoff, now);

			return [...new Set([...resumed, ...expired])];
		}).pipe(Effect.mapError((cause) => new ThreadRetentionFailure({ cause })));
		const AwaitFlight = (
			current: Deferred.Deferred<Exit.Exit<ReadonlyArray<string>, ThreadRetentionFailure>>,
		) =>
			Deferred.await(current).pipe(
				Effect.flatMap((exit) =>
					Exit.isSuccess(exit)
						? Effect.succeed(exit.value)
						: Effect.failCause(exit.cause),
				),
			);
		const RunCleanup = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const next =
					yield* Deferred.make<
						Exit.Exit<ReadonlyArray<string>, ThreadRetentionFailure>
					>();
				const active = yield* Ref.modify(flight, (current) =>
					current === undefined ? ([next, next] as const) : ([current, current] as const),
				);
				if (active !== next) return yield* restore(AwaitFlight(active));

				yield* Effect.forkIn(
					restore(Cleanup).pipe(
						Effect.onExit((exit) =>
							Ref.modify(flight, (current) =>
								current === next
									? ([undefined, undefined] as const)
									: ([undefined, current] as const),
							).pipe(
								Effect.andThen(Deferred.succeed(next, exit)),
								Effect.uninterruptible,
							),
						),
					),
					service_scope,
				);
				return yield* restore(AwaitFlight(next));
			}),
		);

		yield* Effect.addFinalizer(() => Scope.close(service_scope, Exit.void));
		yield* Effect.forkIn(
			RunCleanup.pipe(
				Effect.asVoid,
				Effect.catchCause((cause) =>
					Effect.logWarning("Thread retention startup cleanup failed", { cause }),
				),
			),
			service_scope,
		);
		yield* Effect.forkIn(
			scheduler
				.Schedule(
					RunCleanup.pipe(
						Effect.asVoid,
						Effect.catch(() => Effect.void),
					),
				)
				.pipe(Scope.provide(service_scope)),
			service_scope,
		);
		yield* Effect.yieldNow;

		return { RunCleanup };
	}),
);
