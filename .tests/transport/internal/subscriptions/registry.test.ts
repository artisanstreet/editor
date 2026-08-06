import { Effect, Exit, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { InboundControlEnvelope, OutboundControlEnvelope } from "@artisan/protocol";

import {
	SubscriptionErrorReporter,
	SubscriptionIdentity,
	SubscriptionOptions,
	SubscriptionProtocol,
} from "../../../../modules/transport/src/internal/subscriptions/context";
import { make_client_subscription_coordinator } from "../../../../modules/transport/src/internal/subscriptions/coordinator";

const make_test_context = () => {
	const sent: Array<InboundControlEnvelope> = [];
	let tick = 0;

	const layer = Layer.mergeAll(
		Layer.succeed(SubscriptionOptions, { event_capacity: 16, subscription_capacity: 16 }),
		Layer.succeed(SubscriptionIdentity, {
			make_id: (prefix: string) =>
				Effect.sync(() => {
					tick += 1;

					return `${prefix}_${tick}`;
				}),
			make_trace: Effect.sync(() => {
				tick += 1;

				return {
					message_id: `message_${tick}`,
					origin: "frontend" as const,
					protocol_version: 1 as const,
					schema_version: 1 as const,
					sent_at: "2026-08-06T00:00:00.000Z",
				};
			}),
		}),
		Layer.succeed(SubscriptionProtocol, {
			send_current: (envelope: InboundControlEnvelope) =>
				Effect.sync(() => {
					sent.push(envelope);
				}),
		}),
		Layer.succeed(SubscriptionErrorReporter, { publish_error: () => Effect.void }),
	);

	const started_envelope = (
		correlation_id: string,
		subscription_id: string,
		stream_id: string,
	): Extract<OutboundControlEnvelope, { kind: "subscription.started" }> => ({
		correlation_id,
		kind: "subscription.started",
		message_id: `backend_${(tick += 1)}`,
		origin: "backend",
		payload: { stream_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-06T00:00:00.000Z",
		subscription_id,
	});

	const sent_subscribe = (index: number) => {
		const envelope = sent[index];

		if (envelope?.kind !== "subscribe") {
			throw new Error("expected a subscribe envelope");
		}

		return envelope;
	};

	return { layer, sent, sent_subscribe, started_envelope };
};

const until_sent = (sent: ReadonlyArray<InboundControlEnvelope>, count: number) =>
	Effect.gen(function* () {
		while (sent.length < count) {
			yield* Effect.sleep("1 millis");
		}
	}).pipe(Effect.timeout("2 seconds"));

describe("client subscription registry connection reset", () => {
	it("preserves a pending start across a reset so its subscriber resumes", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const subscriber = yield* coordinator.SubscribeThreadList.pipe(
						Effect.forkScoped,
					);

					yield* until_sent(context.sent, 1);
					/**
					 * The session dies before the backend answers. The reset
					 * must keep the pending readiness gate: its subscriber is
					 * blocked on that exact deferred, and only the replacement
					 * session's answer can release it.
					 */
					yield* coordinator.ResetConnection;

					const subscribe = context.sent_subscribe(0);
					yield* coordinator.HandleStarted(
						context.started_envelope(
							subscribe.message_id,
							subscribe.subscription_id,
							"stream_after_reset",
						),
					);
					yield* Fiber.join(subscriber).pipe(Effect.timeout("2 seconds"));
				}),
			),
		);
	});

	it("accepts the replacement session's resent start after a reset", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const subscriber = yield* coordinator.SubscribeThreadList.pipe(
						Effect.forkScoped,
					);

					yield* until_sent(context.sent, 1);
					const subscribe = context.sent_subscribe(0);
					yield* coordinator.HandleStarted(
						context.started_envelope(
							subscribe.message_id,
							subscribe.subscription_id,
							"stream_first_session",
						),
					);
					yield* Fiber.join(subscriber);

					/**
					 * A new session resends the same envelope after a reset. Its
					 * answer must register as a fresh start — never a duplicate,
					 * which would kill the whole transport session.
					 */
					yield* coordinator.ResetConnection;
					const restarted = yield* Effect.exit(
						coordinator.HandleStarted(
							context.started_envelope(
								subscribe.message_id,
								subscribe.subscription_id,
								"stream_second_session",
							),
						),
					);

					expect(Exit.isSuccess(restarted)).toBe(true);
					yield* coordinator.AwaitReady.pipe(Effect.timeout("2 seconds"));
				}),
			),
		);
	});
});
