import { Context, Effect, Ref } from "effect";

import {
	type AckEnvelope,
	type EventEnvelope,
	type HeartbeatPongEnvelope,
	type OutboundControlEnvelope,
	type ReplayEnvelope,
} from "@artisan/protocol";

import { JournalStore } from "../../persistence/journal-store";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	type ConnectionState,
	CursorsToRecord,
	type ReadyState,
	RecordToCursors,
} from "../connection-state";

export class ConnectionSubscriptionControl extends Context.Service<
	ConnectionSubscriptionControl,
	{
		readonly state: Ref.Ref<ConnectionState>;
		readonly Enqueue: (envelope: OutboundControlEnvelope) => Effect.Effect<void>;
		readonly EnqueueError: (
			current: ConnectionState,
			code: string,
			message: string,
			retryable: boolean,
			correlation_id?: string,
		) => Effect.Effect<void>;
	}
>()("Artisan/ConnectionSubscriptionControl") {}

export const MakeSubscriptionControlHandlers = Effect.gen(function* () {
	const journal = yield* JournalStore;
	const metadata = yield* RuntimeMetadata;
	const { state, Enqueue, EnqueueError } = yield* ConnectionSubscriptionControl;
	const EnqueueReplayEvents = (events: ReadonlyArray<EventEnvelope>) =>
		Effect.gen(function* () {
			const current = yield* Ref.get(state);
			if (current._tag !== "Ready") return;
			yield* Effect.forEach(events, Enqueue, { discard: true });
			yield* Ref.set(state, {
				...current,
				delivered_cursors: events.reduce<Readonly<Record<string, number>>>(
					(cursors, event) => ({
						...cursors,
						[event.stream_id]: Math.max(cursors[event.stream_id] ?? 0, event.sequence),
					}),
					current.delivered_cursors,
				),
				delivered_journal_sequence: events.reduce(
					(sequence, event) => Math.max(sequence, event.journal_sequence),
					current.delivered_journal_sequence,
				),
			});
		});

	const HandleAck = (ack: AckEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			const invalid_journal =
				ack.payload.journal_sequence < current.acknowledged_journal_sequence ||
				ack.payload.journal_sequence > current.delivered_journal_sequence;
			const invalid_stream = ack.payload.event_cursors.some((cursor) => {
				const acknowledged = current.acknowledged_cursors[cursor.stream_id] ?? 0;
				const delivered = current.delivered_cursors[cursor.stream_id] ?? 0;

				return cursor.sequence < acknowledged || cursor.sequence > delivered;
			});

			if (invalid_journal || invalid_stream) {
				yield* EnqueueError(
					current,
					"protocol.invalid_ack",
					"The acknowledgement is outside the delivered range.",
					false,
					ack.message_id,
				);
				return;
			}

			const valid_replay_point = yield* journal
				.ValidateReplayPoint({
					after_journal_sequence: ack.payload.journal_sequence,
					stream_cursors: ack.payload.event_cursors,
				})
				.pipe(
					Effect.as(true),
					Effect.catch(() => Effect.succeed(false)),
				);

			if (!valid_replay_point) {
				yield* EnqueueError(
					current,
					"protocol.invalid_ack",
					"The acknowledgement does not identify a durable replay point.",
					false,
					ack.message_id,
				);
				return;
			}

			yield* Ref.set(state, {
				...current,
				acknowledged_cursors: {
					...current.acknowledged_cursors,
					...CursorsToRecord(ack.payload.event_cursors),
				},
				acknowledged_journal_sequence: ack.payload.journal_sequence,
			});
		});

	const HandleReplay = (replay: ReplayEnvelope, current: ReadyState) =>
		journal
			.ReadReplay({
				after_journal_sequence: replay.payload.after_journal_sequence,
				...(replay.payload.event_cursors
					? { stream_cursors: replay.payload.event_cursors }
					: {}),
			})
			.pipe(
				Effect.flatMap((events) =>
					Effect.gen(function* () {
						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;

						yield* EnqueueReplayEvents(events);
						const updated = yield* Ref.get(state);
						if (updated._tag !== "Ready") return;

						yield* Enqueue({
							correlation_id: replay.message_id,
							kind: "replay.complete",
							message_id,
							origin: "backend",
							payload: {
								current_event_cursors: RecordToCursors(updated.delivered_cursors),
								journal_sequence: updated.delivered_journal_sequence,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
					}),
				),
				Effect.catch(() =>
					EnqueueError(
						current,
						"protocol.replay_invalid",
						"The replay cursor does not match the journal.",
						false,
						replay.message_id,
					),
				),
			);

	const HandlePong = (pong: HeartbeatPongEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			const pending = current.pending_heartbeat;
			const matches =
				pending?.message_id === pong.correlation_id && pending.nonce === pong.payload.nonce;

			if (!matches) {
				yield* EnqueueError(
					current,
					"protocol.invalid_heartbeat",
					"The heartbeat response does not match an active ping.",
					false,
					pong.message_id,
				);
				return;
			}

			const { pending_heartbeat: _, ...without_pending } = current;
			yield* Ref.set(state, without_pending);
		});

	return { HandleAck, HandlePong, HandleReplay };
});
