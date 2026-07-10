import { Context, Effect, Layer, PubSub, Scope } from "effect";

/** Broadcasts durable journal commits as wakeup hints for live connections. */
export class JournalNotifier extends Context.Service<
	JournalNotifier,
	{
		readonly Publish: (journal_sequence: number) => Effect.Effect<void>;
		readonly Subscribe: Effect.Effect<PubSub.Subscription<number>, never, Scope.Scope>;
	}
>()("Artisan/JournalNotifier") {}

export const JournalNotifierLive = Layer.effect(
	JournalNotifier,
	Effect.gen(function* () {
		const pubsub = yield* Effect.acquireRelease(PubSub.unbounded<number>(), PubSub.shutdown);

		return {
			Publish: (journal_sequence) =>
				PubSub.publish(pubsub, journal_sequence).pipe(Effect.asVoid),
			Subscribe: PubSub.subscribe(pubsub),
		};
	}),
);
