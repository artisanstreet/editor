import { describe, expect, it } from "vitest";
import { Deferred, Effect, Fiber, Ref } from "effect";
import { TestClock } from "effect/testing";

import { WatchEngineInactivity } from "../../modules/engines/src/process/inactivity-deadline";

const inactivity_ms = 60_000;

/**
 * Drives one watcher against a scripted activity counter. `Closed` never
 * settles unless a case resolves it, so the deadline is the only thing that
 * can end the run.
 */
const watch = <A>(
	Scenario: (input: {
		readonly activity: Ref.Ref<number>;
		readonly closed: Deferred.Deferred<"completed">;
		readonly expecting: Ref.Ref<boolean>;
		readonly stalled: Ref.Ref<boolean>;
	}) => Effect.Effect<A>,
) =>
	Effect.runPromise(
		Effect.gen(function* () {
			const activity = yield* Ref.make(0);
			const closed = yield* Deferred.make<"completed">();
			const expecting = yield* Ref.make(true);
			const stalled = yield* Ref.make(false);
			const watcher = yield* WatchEngineInactivity({
				Activity: Ref.get(activity),
				Closed: Deferred.await(closed),
				Expecting: Ref.get(expecting),
				inactivity_ms,
				OnStall: Ref.set(stalled, true),
			}).pipe(Effect.forkChild);

			const result = yield* Scenario({ activity, closed, expecting, stalled });

			yield* Fiber.interrupt(watcher);

			return result;
		}).pipe(Effect.provide(TestClock.layer())),
	);

describe("engine inactivity deadline", () => {
	it("settles a run that produces nothing for a whole window", async () => {
		const stalled = await watch(({ stalled }) =>
			Effect.gen(function* () {
				yield* TestClock.adjust(inactivity_ms);

				return yield* Ref.get(stalled);
			}),
		);

		expect(stalled).toBe(true);
	});

	it("holds off while the window is still open", async () => {
		const stalled = await watch(({ stalled }) =>
			Effect.gen(function* () {
				yield* TestClock.adjust(inactivity_ms / 2);

				return yield* Ref.get(stalled);
			}),
		);

		expect(stalled).toBe(false);
	});

	/**
	 * The regression that matters: a long run that keeps producing must never
	 * be settled, however long it runs in total. The old deadline was a total
	 * budget and would have fired here.
	 */
	it("never settles a run that keeps producing", async () => {
		const stalled = await watch(({ activity, stalled }) =>
			Effect.gen(function* () {
				for (let observation = 1; observation <= 40; observation += 1) {
					yield* Ref.set(activity, observation);
					yield* TestClock.adjust(inactivity_ms / 2);
				}

				return yield* Ref.get(stalled);
			}),
		);

		expect(stalled).toBe(false);
	});

	it("re-arms after an observation and settles on the next full silence", async () => {
		const stalled = await watch(({ activity, stalled }) =>
			Effect.gen(function* () {
				yield* TestClock.adjust(inactivity_ms / 2);
				yield* Ref.set(activity, 1);
				yield* TestClock.adjust(inactivity_ms / 2);

				const survived = yield* Ref.get(stalled);

				yield* TestClock.adjust(inactivity_ms);

				return { settled: yield* Ref.get(stalled), survived };
			}),
		);

		expect(stalled).toEqual({ settled: true, survived: false });
	});

	/** A session that outlives its turns is legitimately silent between them. */
	it("ignores silence while the run owes no output", async () => {
		const stalled = await watch(({ expecting, stalled }) =>
			Effect.gen(function* () {
				yield* Ref.set(expecting, false);
				yield* TestClock.adjust(inactivity_ms * 4);

				return yield* Ref.get(stalled);
			}),
		);

		expect(stalled).toBe(false);
	});

	it("stops watching once the run reaches a terminal state", async () => {
		const stalled = await watch(({ closed, stalled }) =>
			Effect.gen(function* () {
				yield* Deferred.succeed(closed, "completed");
				yield* TestClock.adjust(inactivity_ms * 2);

				return yield* Ref.get(stalled);
			}),
		);

		expect(stalled).toBe(false);
	});
});
