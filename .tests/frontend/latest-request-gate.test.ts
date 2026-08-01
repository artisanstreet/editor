import { Deferred, Effect, Fiber, Ref } from "effect";
import { describe, expect, it } from "vitest";

import { MakeLatestRequestGate } from "../../modules/frontend/src/lib/lifecycle/latest-request-gate";

describe("latest request gate", () => {
	it("rejects A when B publishes before A completes", async () => {
		const published = await Effect.runPromise(
			Effect.gen(function* () {
				const gate = yield* MakeLatestRequestGate;
				const result = yield* Ref.make("none");
				const release_a = yield* Deferred.make<void>();
				const release_b = yield* Deferred.make<void>();

				const Run = (value: "A" | "B", release: typeof release_a) =>
					Effect.gen(function* () {
						const generation = yield* gate.Begin;
						yield* Deferred.await(release);
						if (yield* gate.IsCurrent(generation)) yield* Ref.set(result, value);
					});

				const fiber_a = yield* Effect.forkChild(Run("A", release_a));
				yield* Effect.yieldNow;
				const fiber_b = yield* Effect.forkChild(Run("B", release_b));
				yield* Effect.yieldNow;
				yield* Deferred.succeed(release_b, undefined);
				yield* Deferred.succeed(release_a, undefined);
				yield* Fiber.await(fiber_b);
				yield* Fiber.await(fiber_a);
				return yield* Ref.get(result);
			}),
		);

		expect(published).toBe("B");
	});
});
