import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Ref, Stream } from "effect";
import { TestClock } from "effect/testing";

import { RunConversationSubscription } from "../../modules/frontend/src/lib/conversation/subscription";

describe("conversation subscription", () => {
	it.effect("resubscribes and applies a later projection update after the live stream ends", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const received = yield* Ref.make<ReadonlyArray<string>>([]);
			const recoveries = yield* Ref.make(0);

			const subscribe = Effect.gen(function* () {
				const attempt = yield* Ref.getAndUpdate(attempts, (current) => current + 1);
				if (attempt === 0) return Stream.empty;

				return Stream.concat(Stream.succeed("later durable patch"), Stream.never);
			});
			const fiber = yield* RunConversationSubscription(
				subscribe,
				(update) => Ref.update(received, (current) => [...current, update]),
				Ref.update(recoveries, (current) => current + 1),
			).pipe(Effect.forkScoped);

			yield* TestClock.adjust("100 millis");

			expect(yield* Ref.get(attempts)).toBe(2);
			expect(yield* Ref.get(recoveries)).toBe(1);
			expect(yield* Ref.get(received)).toEqual(["later durable patch"]);
			yield* Fiber.interrupt(fiber);
		}).pipe(Effect.provide(TestClock.layer())),
	);
});
