import { Deferred, Effect, Fiber, Layer, Queue, Ref } from "effect";
import { describe, expect, it } from "vitest";

import {
	ThreadRetention,
	ThreadRetentionClock,
	ThreadRetentionFailure,
	ThreadRetentionLive,
	ThreadRetentionScheduler,
} from "../../modules/backend/src/threads/thread-retention";
import {
	ThreadErasure,
	ThreadErasureFailure,
} from "../../modules/backend/src/threads/thread-erasure";
import { ThreadRetentionPolicyService } from "../../modules/backend/src/threads/thread-retention-policy";

describe("thread retention startup loading", () => {
	it("constructs while startup cleanup is held and coalesces startup, manual, and scheduler cleanup", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const scheduled = await Effect.runPromise(Deferred.make<void>());
		const ticks = await Effect.runPromise(Queue.unbounded<void>());
		const resume_calls = await Effect.runPromise(Ref.make(0));
		const cleanup_calls = await Effect.runPromise(Ref.make(0));
		const layer = ThreadRetentionLive.pipe(
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionClock, {
					Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
				}),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionPolicyService, {
					Read: Effect.succeed({ enabled: true, inactivity_days: 7 }),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadErasure, {
					CleanupExpired: () =>
						Ref.update(cleanup_calls, (count) => count + 1).pipe(
							Effect.as(["expired"]),
						),
					ResumeClaimed: () =>
						Ref.update(resume_calls, (count) => count + 1).pipe(
							Effect.andThen(Deferred.succeed(started, undefined)),
							Effect.andThen(Deferred.await(release)),
							Effect.as(["resumed"]),
						),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionScheduler, {
					Schedule: (task) =>
						Deferred.succeed(scheduled, undefined).pipe(
							Effect.andThen(
								Effect.forever(Queue.take(ticks).pipe(Effect.andThen(task))),
							),
						),
				}),
			),
		);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const retention = yield* ThreadRetention;
					yield* Deferred.await(started);
					yield* Deferred.await(scheduled);
					const manual = yield* retention.RunCleanup.pipe(Effect.forkScoped);
					yield* Queue.offer(ticks, undefined);
					yield* Effect.yieldNow;
					yield* Deferred.succeed(release, undefined);
					return yield* Fiber.join(manual);
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(result).toEqual(["resumed", "expired"]);
		expect(await Effect.runPromise(Ref.get(resume_calls))).toBe(1);
		expect(await Effect.runPromise(Ref.get(cleanup_calls))).toBe(1);
	});

	it("settles a failed startup flight for joiners and retries from a cleared identity", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const calls = await Effect.runPromise(Ref.make(0));
		const layer = ThreadRetentionLive.pipe(
			Layer.provideMerge(Layer.succeed(ThreadRetentionClock, { Now: Effect.succeed("now") })),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionPolicyService, {
					Read: Effect.succeed({ enabled: false, inactivity_days: 7 }),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadErasure, {
					CleanupExpired: () => Effect.die("disabled cleanup must not run"),
					ResumeClaimed: () =>
						Ref.updateAndGet(calls, (count) => count + 1).pipe(
							Effect.flatMap((count) =>
								count === 1
									? Deferred.succeed(started, undefined).pipe(
											Effect.andThen(Deferred.await(release)),
											Effect.andThen(
												Effect.fail(
													new ThreadErasureFailure({
														cause: new Error("first failure"),
													}),
												),
											),
										)
									: Effect.succeed(["retried"]),
							),
						),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionScheduler, { Schedule: () => Effect.never }),
			),
		);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const retention = yield* ThreadRetention;
					yield* Deferred.await(started);
					const joined = yield* retention.RunCleanup.pipe(Effect.forkScoped);
					yield* Effect.yieldNow;
					yield* Deferred.succeed(release, undefined);
					const failure = yield* Fiber.join(joined).pipe(Effect.flip);
					const retried = yield* retention.RunCleanup;
					return { failure, retried };
				}).pipe(Effect.provide(layer)),
			),
		);

		expect(result.failure).toBeInstanceOf(ThreadRetentionFailure);
		expect(result.failure.cause).toBeInstanceOf(ThreadErasureFailure);
		expect(result.retried).toEqual(["retried"]);
		expect(await Effect.runPromise(Ref.get(calls))).toBe(2);
	});

	it("interrupts held startup and joiners on scope close without late cleanup", async () => {
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		const interrupted = await Effect.runPromise(Ref.make(0));
		const cleanup_calls = await Effect.runPromise(Ref.make(0));
		const layer = ThreadRetentionLive.pipe(
			Layer.provideMerge(Layer.succeed(ThreadRetentionClock, { Now: Effect.succeed("now") })),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionPolicyService, {
					Read: Effect.succeed({ enabled: true, inactivity_days: 7 }),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadErasure, {
					CleanupExpired: () =>
						Ref.update(cleanup_calls, (count) => count + 1).pipe(Effect.as([])),
					ResumeClaimed: () =>
						Deferred.succeed(started, undefined).pipe(
							Effect.andThen(Deferred.await(release)),
							Effect.onInterrupt(() => Ref.update(interrupted, (count) => count + 1)),
						),
				} as never),
			),
			Layer.provideMerge(
				Layer.succeed(ThreadRetentionScheduler, { Schedule: () => Effect.never }),
			),
		);

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const retention = yield* ThreadRetention;
					yield* Deferred.await(started);
					yield* retention.RunCleanup.pipe(
						Effect.onInterrupt(() => Ref.update(interrupted, (count) => count + 1)),
						Effect.forkScoped,
					);
					yield* Effect.yieldNow;
				}).pipe(Effect.provide(layer)),
			),
		);
		await Effect.runPromise(Deferred.succeed(release, undefined));

		expect(await Effect.runPromise(Ref.get(interrupted))).toBe(2);
		expect(await Effect.runPromise(Ref.get(cleanup_calls))).toBe(0);
	});
});
