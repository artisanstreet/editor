import { Data, Duration, Effect, Schedule, Scope, Stream } from "effect";

class AuthoritativeSubscriptionLost extends Data.TaggedError("AuthoritativeSubscriptionLost")<{
	readonly message: string;
}> {}

const AuthoritativeSubscriptionRetrySchedule = Schedule.exponential("100 millis").pipe(
	Schedule.modifyDelay(({ duration }) =>
		Effect.succeed(Duration.min(duration, Duration.seconds(5))),
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
) => {
	const Attempt = Effect.scoped(
		subscribe.pipe(
			Effect.flatMap((updates) =>
				Stream.runForEach(updates, on_update).pipe(
					Effect.flatMap(() =>
						Effect.fail(
							new AuthoritativeSubscriptionLost({
								message: "Authoritative subscription ended unexpectedly.",
							}),
						),
					),
				),
			),
		),
	).pipe(Effect.tapError(() => on_recover.pipe(Effect.catch(() => Effect.void))));

	return Attempt.pipe(
		Effect.retry({ schedule: AuthoritativeSubscriptionRetrySchedule }),
		Effect.asVoid,
	);
};

export const RunConversationSubscription = RunAuthoritativeSubscription;
