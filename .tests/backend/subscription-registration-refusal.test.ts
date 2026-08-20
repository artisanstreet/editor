import { Deferred, Effect, Layer, Ref, Semaphore } from "effect";
import { describe, expect, it } from "vitest";

import type { OutboundControlEnvelope } from "@artisan/protocol";

import type {
	ConnectionState,
	PendingSubscription,
	ProjectionSubscription,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import { ConnectionSubscriptionControl } from "../../modules/backend/src/protocol/subscriptions/control";
import { MakeConnectionProjectionRuntime } from "../../modules/backend/src/protocol/subscriptions/runtime";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const subscription: ProjectionSubscription = {
	_tag: "thread.list",
	sequence: 0,
	stream_id: "thread_list_stream",
};

const ready_state = (
	pending_subscriptions: Readonly<Record<string, PendingSubscription>>,
): ReadyState => ({
	_tag: "Ready",
	acknowledged_cursors: {},
	acknowledged_journal_sequence: 0,
	connection_id: "connection_1",
	delivered_cursors: {},
	delivered_journal_sequence: 0,
	last_activity_ms: 0,
	pending_subscriptions,
	stream_ticket: "ticket_1",
	subscriptions: {},
});

const make_claim = Effect.gen(function* () {
	const cancelled = yield* Deferred.make<void>();
	const released = yield* Deferred.make<void>();
	const publication = yield* Semaphore.make(1);

	return { cancelled, message_id: "subscribe_message_1", publication, released };
});

/**
 * Drives the projection runtime against a connection whose registration will be
 * refused, and reports every envelope the connection would have sent.
 */
const refused_registration = (state_for: (claim: PendingSubscription) => ConnectionState) =>
	Effect.gen(function* () {
		const claim = yield* make_claim;
		const enqueued: Array<OutboundControlEnvelope> = [];
		const errors: Array<{ readonly code: string; readonly retryable: boolean }> = [];
		const state = yield* Ref.make(state_for(claim));
		const runtime = yield* MakeConnectionProjectionRuntime.pipe(
			Effect.provide(
				Layer.mergeAll(
					Layer.succeed(ConnectionSubscriptionControl, {
						Enqueue: (envelope: OutboundControlEnvelope) =>
							Effect.sync(() => {
								enqueued.push(envelope);
							}),
						EnqueueError: (
							_current: ConnectionState,
							code: string,
							_message: string,
							retryable: boolean,
							correlation_id?: string,
						) =>
							Effect.sync(() => {
								errors.push({ code, retryable });
								enqueued.push({
									correlation_id,
									kind: "error",
									message_id: "error_1",
									origin: "backend",
									payload: { code, retryable },
									protocol_version: 1,
									schema_version: 1,
									sent_at: "2026-08-16T00:00:00.000Z",
								} as unknown as OutboundControlEnvelope);
							}),
						state,
					}),
					Layer.succeed(RuntimeMetadata, {
						instance_id: "backend_1",
						MakeId: () => Effect.succeed("message_1"),
						Now: Effect.succeed("2026-08-16T00:00:00.000Z"),
					}),
				),
			),
		);

		yield* runtime.Start(
			claim.message_id,
			"subscription_1",
			claim,
			ready_state({ subscription_1: claim }),
			subscription,
			() => ({}) as OutboundControlEnvelope,
		);

		return { enqueued, errors };
	});

describe("subscription registration refusal", () => {
	/**
	 * A refused registration used to return without sending anything at all.
	 * The subscriber is parked on `subscription.started`, and a park is not a
	 * failure, so no retry schedule could rescue it — the thread list simply
	 * stayed empty until the window was reloaded.
	 */
	it("answers a superseded claim with a retryable error instead of silence", async () => {
		const { enqueued, errors } = await Effect.runPromise(
			/** The claim is gone from pending: another subscribe took the id. */
			refused_registration(() => ready_state({})),
		);

		expect(errors).toEqual([{ code: "subscription.unavailable", retryable: true }]);
		expect(enqueued.some((envelope) => envelope.kind === "subscription.started")).toBe(false);
	});

	it("answers a subscribe that arrives after the connection closed", async () => {
		const { enqueued, errors } = await Effect.runPromise(
			refused_registration(() => ({ _tag: "Closed" })),
		);

		expect(errors).toEqual([{ code: "subscription.unavailable", retryable: true }]);
		expect(enqueued.some((envelope) => envelope.kind === "subscription.started")).toBe(false);
	});

	it("still starts a subscription whose claim is intact", async () => {
		const { enqueued, errors } = await Effect.runPromise(
			refused_registration((claim) => ready_state({ subscription_1: claim })),
		);

		expect(errors).toEqual([]);
		expect(enqueued.some((envelope) => envelope.kind === "subscription.started")).toBe(true);
	});
});
