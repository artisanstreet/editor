import { Effect, Ref } from "effect";

/**
 * Component-local ownership for reads that may complete out of order.
 * Callers publish a result only while its generation is still current.
 */
export const MakeLatestRequestGate = Effect.gen(function* () {
	const generation = yield* Ref.make(0);

	const Begin = Effect.gen(function* () {
		return yield* Ref.updateAndGet(generation, (current) => current + 1);
	});

	const IsCurrent = (candidate: number) =>
		Effect.gen(function* () {
			return (yield* Ref.get(generation)) === candidate;
		});

	return { Begin, IsCurrent } as const;
});
