import { Data, Duration, Effect, Schedule, Scope, Stream } from "effect";

class AuthoritativeSubscriptionLost extends Data.TaggedError("AuthoritativeSubscriptionLost")<{
	readonly message: string;
}> {}

const AuthoritativeSubscriptionRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.modifyDelay(({ duration }) =>
		Effect.gen(function* () {
			return Duration.min(duration, Duration.seconds(5));
		}),
	),
);

/**
 * Keeps one authoritative projection stream alive for the lifetime of its owning scope.
 *
 * Each retry receives its own scope so a failed queue is finalized before a new
 * subscription is registered. Recovery first resyncs the durable snapshot, then
 * retries with capped exponential backoff.
 */
export const RunAuthoritativeSubscription = <Update, SubscribeError, StreamError>(
	subscribe: Effect.Effect<Stream.Stream<Update, StreamError>, SubscribeError, Scope.Scope>,
	on_update: (update: Update) => Effect.Effect<void>,
	on_recover: Effect.Effect<void>,
) =>
	Effect.gen(function* () {
		const Attempt = Effect.gen(function* () {
			yield* Effect.scoped(
				Effect.gen(function* () {
					const updates = yield* subscribe;
					yield* Stream.runForEach(updates, on_update);
					return yield* Effect.fail(
						new AuthoritativeSubscriptionLost({
							message: "Authoritative subscription ended unexpectedly.",
						}),
					);
				}),
			);
		}).pipe(
			Effect.tapError(() =>
				Effect.gen(function* () {
					yield* on_recover.pipe(
						Effect.catch(() =>
							Effect.gen(function* () {
								return;
							}),
						),
					);
				}),
			),
		);

		yield* Attempt.pipe(Effect.retry({ schedule: AuthoritativeSubscriptionRetrySchedule }));
	});

export const RunConversationSubscription = RunAuthoritativeSubscription;
