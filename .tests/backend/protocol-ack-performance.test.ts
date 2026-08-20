import { Chunk, Effect, Layer, Ref } from "effect";
import { describe, expect, it } from "vitest";

import type { AckEnvelope, EventEnvelope, OutboundControlEnvelope } from "@artisan/protocol";

import type {
	ConnectionState,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import {
	ConnectionSubscriptionControl,
	MakeSubscriptionControlHandlers,
} from "../../modules/backend/src/protocol/subscriptions/control";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const Event = (journal_sequence: number, stream_id: string, sequence: number) =>
	({ journal_sequence, sequence, stream_id }) as EventEnvelope;

const Ack = (
	message_id: string,
	journal_sequence: number,
	event_cursors: AckEnvelope["payload"]["event_cursors"],
) =>
	({
		kind: "ack",
		message_id,
		origin: "frontend",
		payload: { event_cursors, journal_sequence },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-15T00:00:00.000Z",
	}) as AckEnvelope;

describe("subscription acknowledgement replay points", () => {
	it("rejects journal gaps and advances exact partial/final ACKs without a journal replay", async () => {
		const calls = { replay: 0, validate_replay_point: 0 };
		const outbound: Array<OutboundControlEnvelope> = [];
		const initial: ReadyState = {
			_tag: "Ready",
			acknowledged_cursors: { alpha: 1 },
			acknowledged_journal_sequence: 10,
			connection_id: "connection_ack_performance",
			delivered_cursors: { alpha: 3 },
			delivered_journal_sequence: 13,
			last_activity_ms: 0,
			stream_ticket: "ticket_ack_performance",
			subscriptions: {},
			unacknowledged_events: Chunk.make(Event(11, "alpha", 2), Event(13, "alpha", 3)),
		};
		const state = await Effect.runPromise(Ref.make<ConnectionState>(initial));
		const JournalLive = Layer.succeed(JournalStore, {
			ReadReplay: () =>
				Effect.sync(() => {
					calls.replay += 1;
					return [];
				}),
			ValidateReplayPoint: () =>
				Effect.sync(() => {
					calls.validate_replay_point += 1;
				}),
		} as never);
		const ControlLive = Layer.succeed(ConnectionSubscriptionControl, {
			Enqueue: (envelope: OutboundControlEnvelope) =>
				Effect.sync(() => {
					outbound.push(envelope);
				}),
			EnqueueError: (current, code, message, retryable, correlation_id) =>
				Effect.sync(() => {
					outbound.push({
						correlation_id,
						kind: "protocol.error",
						message_id: "ack_error",
						origin: "backend",
						payload: { code, message, retryable },
						protocol_version: 1,
						schema_version: 1,
						sent_at: "2026-08-15T00:00:00.000Z",
					});
				}),
			state,
		});
		const MetadataLive = Layer.succeed(RuntimeMetadata, {
			instance_id: "ack_performance",
			MakeId: (prefix) => Effect.succeed(`${prefix}_ack_performance`),
			Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
		});

		await Effect.gen(function* () {
			const handlers = yield* MakeSubscriptionControlHandlers;
			yield* handlers.HandleAck(
				Ack("ack_gap", 12, [{ sequence: 2, stream_id: "alpha" }]),
				initial,
			);
			yield* handlers.HandleAck(
				Ack("ack_partial", 11, [{ sequence: 2, stream_id: "alpha" }]),
				initial,
			);
			const partial = yield* Ref.get(state);
			if (partial._tag !== "Ready") throw new Error("expected a ready connection");
			expect(partial.acknowledged_cursors).toEqual({ alpha: 2 });
			expect(Chunk.toArray(partial.unacknowledged_events ?? Chunk.empty())).toEqual([
				Event(13, "alpha", 3),
			]);

			yield* handlers.HandleAck(
				Ack("ack_final", 13, [{ sequence: 3, stream_id: "alpha" }]),
				/** The handler must advance from the Ref, not this stale dispatch snapshot. */
				initial,
			);
		})
			.pipe(Effect.provide(Layer.mergeAll(JournalLive, ControlLive, MetadataLive)))
			.pipe(Effect.runPromise);

		expect(calls).toEqual({ replay: 0, validate_replay_point: 0 });
		expect(outbound).toMatchObject([
			{
				correlation_id: "ack_gap",
				kind: "protocol.error",
				payload: { code: "protocol.invalid_ack" },
			},
		]);
		const final = await Effect.runPromise(Ref.get(state));
		expect(final).toMatchObject({
			acknowledged_cursors: { alpha: 3 },
			acknowledged_journal_sequence: 13,
			unacknowledged_events: Chunk.empty(),
		});
	});
});
