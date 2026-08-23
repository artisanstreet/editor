import { Cause, Deferred, Effect, Exit, Fiber, Layer, Stream } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import type { InboundControlEnvelope, OutboundControlEnvelope } from "@artisan/protocol";

import {
	SubscriptionErrorReporter,
	SubscriptionIdentity,
	SubscriptionProtocol,
} from "../../../../modules/transport/src/internal/subscriptions/context";
import { make_client_subscription_coordinator } from "../../../../modules/transport/src/internal/subscriptions/coordinator";
import { event_observer_queue_capacity } from "../../../../modules/transport/src/internal/subscriptions/ingress";
import { projection_subscription_queue_capacity } from "../../../../modules/transport/src/internal/subscriptions/registry";

const make_test_context = () => {
	const sent: Array<InboundControlEnvelope> = [];
	const errors: Array<unknown> = [];
	let tick = 0;

	const layer = Layer.mergeAll(
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
		Layer.succeed(SubscriptionErrorReporter, {
			publish_error: (error) => Effect.sync(() => void errors.push(error)),
		}),
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

	return { errors, layer, sent, sent_subscribe, started_envelope };
};

const until_sent = (sent: ReadonlyArray<InboundControlEnvelope>, count: number) =>
	Effect.gen(function* () {
		while (sent.length < count) {
			yield* Effect.sleep("1 millis");
		}
	}).pipe(Effect.timeout("2 seconds"));

describe("client subscription registry connection reset", () => {
	it("advances an opted-in conversation cursor before reconnect replay", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const subscriber = yield* coordinator
						.SubscribeConversation("thread_1", {
							conversation_id: "conversation_1",
							last_patch_sequence: 8,
						})
						.pipe(Effect.forkScoped);

					yield* until_sent(context.sent, 1);
					const subscribe = context.sent_subscribe(0);
					const stream_id = "conversation_stream_1";
					yield* coordinator.HandleStarted(
						context.started_envelope(
							subscribe.message_id,
							subscribe.subscription_id,
							stream_id,
						),
					);
					yield* Fiber.join(subscriber);
					yield* coordinator.HandleUpdate({
						journal_sequence: 13,
						kind: "conversation.patch",
						message_id: "conversation_patch_1",
						origin: "backend",
						payload: {
							conversation_id: "conversation_1",
							from_sequence: 8,
							patches: [],
							thread_id: "thread_1",
							to_sequence: 9,
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-13T00:00:00.000Z",
						sequence: 0,
						stream_id,
						subscription_id: subscribe.subscription_id,
					});

					yield* coordinator.ResetConnection;
					yield* coordinator.Retry();
					const replay = context.sent_subscribe(1);

					expect(replay.payload).toMatchObject({
						cursor: {
							conversation_id: "conversation_1",
							last_patch_sequence: 9,
						},
						thread_id: "thread_1",
						type: "conversation",
					});
				}),
			),
		);
	});

	it("keeps snapshot reconnect semantics when a conversation cursor was omitted", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const subscriber = yield* coordinator
						.SubscribeConversation("thread_1")
						.pipe(Effect.forkScoped);

					yield* until_sent(context.sent, 1);
					const subscribe = context.sent_subscribe(0);
					const stream_id = "conversation_stream_1";
					yield* coordinator.HandleStarted(
						context.started_envelope(
							subscribe.message_id,
							subscribe.subscription_id,
							stream_id,
						),
					);
					yield* Fiber.join(subscriber);
					yield* coordinator.HandleUpdate({
						journal_sequence: 13,
						kind: "conversation.snapshot",
						message_id: "conversation_snapshot_1",
						origin: "backend",
						payload: {
							conversation_id: "conversation_1",
							items: [],
							journal_sequence: 13,
							last_patch_sequence: 9,
							schema_version: 1,
							thread_id: "thread_1",
							turns: [],
							updated_at: "2026-08-13T00:00:00.000Z",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-13T00:00:00.000Z",
						sequence: 0,
						stream_id,
						subscription_id: subscribe.subscription_id,
					});

					yield* coordinator.ResetConnection;
					yield* coordinator.Retry();
					const replay = context.sent_subscribe(1);

					expect(replay.payload).toEqual({
						thread_id: "thread_1",
						type: "conversation",
					});
				}),
			),
		);
	});

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

	/**
	 * The backend can accept a subscribe and answer nothing at all — a refused
	 * registration used to return without sending `subscription.started` or an
	 * error. Parking on that answer forever is invisible to the caller's retry
	 * schedule, which only fires on failure, so the projection stayed empty
	 * until the window was reloaded.
	 */
	it("fails an unanswered subscribe at its deadline so the caller can retry", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const subscriber = yield* coordinator.SubscribeThreadList.pipe(
						Effect.exit,
						Effect.forkScoped,
					);

					/** Lets the forked subscriber send before any budget is spent. */
					yield* TestClock.adjust("1 milli");

					expect(context.sent_subscribe(0).kind).toBe("subscribe");

					/** The backend never answers. Only time passes. */
					yield* TestClock.adjust("15 seconds");

					const outcome = yield* Fiber.join(subscriber);

					expect(Exit.isFailure(outcome)).toBe(true);
					const failure = Exit.isFailure(outcome)
						? Cause.squash(outcome.cause)
						: undefined;

					expect(failure).toMatchObject({
						protocol_code: "subscription.timeout",
						retryable: true,
					});
				}),
			).pipe(Effect.provide(TestClock.layer())),
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

	it("fails one stalled projection at its bound without retiring the connection", async () => {
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
					const stream_id = "bounded_projection_stream";
					yield* coordinator.HandleStarted(
						context.started_envelope(
							subscribe.message_id,
							subscribe.subscription_id,
							stream_id,
						),
					);
					const updates = yield* Fiber.join(subscriber);

					for (
						let sequence = 0;
						sequence <= projection_subscription_queue_capacity;
						sequence += 1
					) {
						yield* coordinator.HandleUpdate({
							journal_sequence: sequence,
							kind: "thread.list.snapshot",
							message_id: `bounded_projection_${sequence}`,
							origin: "backend",
							payload: { threads: [] },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-21T00:00:00.000Z",
							sequence,
							stream_id,
							subscription_id: subscribe.subscription_id,
						});
					}

					const outcome = yield* updates.pipe(Stream.runDrain, Effect.exit);
					expect(Exit.isFailure(outcome)).toBe(true);
					if (Exit.isFailure(outcome)) {
						expect(Cause.squash(outcome.cause)).toMatchObject({
							code: "stream_overflow",
							protocol_code: "subscription.overflow",
						});
					}
					expect(context.errors).toHaveLength(1);
					expect(context.sent.some((envelope) => envelope.kind === "unsubscribe")).toBe(
						true,
					);
				}),
			),
		);
	});

	it("fails only a stalled event observer when its private bound fills", async () => {
		const context = make_test_context();

		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const coordinator = yield* make_client_subscription_coordinator.pipe(
						Effect.provide(context.layer),
					);
					const first_started = yield* Deferred.make<void>();
					const release_first = yield* Deferred.make<void>();
					let first = true;
					const observer = yield* coordinator.Events.pipe(
						Stream.tap(() => {
							if (!first) return Effect.void;
							first = false;
							return Deferred.succeed(first_started, undefined).pipe(
								Effect.andThen(Deferred.await(release_first)),
							);
						}),
						Stream.runDrain,
						Effect.exit,
						Effect.forkScoped,
					);
					yield* Effect.yieldNow;

					for (
						let sequence = 1;
						sequence <= event_observer_queue_capacity + 2;
						sequence += 1
					) {
						yield* coordinator.ApplyEvent({
							causation_id: `bounded_event_${sequence}`,
							correlation_id: `bounded_event_${sequence}`,
							journal_sequence: sequence,
							kind: "event",
							message_id: `bounded_event_${sequence}`,
							origin: "backend",
							payload: { type: "thread.erased" },
							protocol_version: 1,
							schema_version: 1,
							sent_at: "2026-08-21T00:00:00.000Z",
							sequence,
							stream_id: "thread:bounded-observer",
							thread_id: "thread_bounded_observer",
						});
						if (sequence === 1) yield* Deferred.await(first_started);
					}

					yield* Deferred.succeed(release_first, undefined);
					const outcome = yield* Fiber.join(observer);
					expect(Exit.isFailure(outcome)).toBe(true);
					if (Exit.isFailure(outcome)) {
						expect(Cause.squash(outcome.cause)).toMatchObject({
							code: "event_overflow",
							protocol_code: "event.observer_overflow",
						});
					}
					/** Observer overflow is local; the protocol session remains usable. */
					expect(context.errors).toEqual([]);
				}),
			),
		);
	});
});
