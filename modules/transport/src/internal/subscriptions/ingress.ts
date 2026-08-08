import { Effect, Queue, Ref } from "effect";
import type { EventEnvelope, OutboundControlEnvelope } from "@artisan/protocol";
import { client_error, cursors_to_record, record_to_cursors } from "../client-common";
import { SubscriptionContext } from "./context";
import type { EventApplication, SubscriptionState } from "./model";

export const MakeEventIngress = Effect.gen(function* () {
	const { state } = yield* SubscriptionContext;
	const cursors = Ref.get(state).pipe(
		Effect.map((current) => ({
			event_cursors: record_to_cursors(current.event_cursors),
			last_journal_sequence: current.last_journal_sequence,
		})),
	);

	/**
	 * Forgets the applied resume position. Used when the position itself has
	 * been implicated in a session failure: resuming with the same cursors
	 * would replay into the identical divergence on every future attempt,
	 * while a fresh bootstrap replays from scratch and heals.
	 */
	const drop_cursors = Ref.update(state, (current) => ({
		...current,
		event_cursors: {},
		last_journal_sequence: 0,
	}));

	const apply_event = (event: EventEnvelope) =>
		Ref.modify<SubscriptionState, EventApplication>(state, (current) => {
			const stream_sequence = current.event_cursors[event.stream_id] ?? 0;

			if (
				event.journal_sequence <= current.last_journal_sequence &&
				event.sequence <= stream_sequence
			) {
				return [{ _tag: "Duplicate" }, current];
			}

			if (event.sequence !== stream_sequence + 1) {
				return [{ _tag: "Gap" }, current];
			}

			if (event.journal_sequence !== current.last_journal_sequence + 1) {
				return [{ _tag: "Gap" }, current];
			}

			/**
			 * Events are optional hot observations. Each observer has its own sliding
			 * queue, so renderer speed can never hold the durable journal cursor or
			 * its ACK hostage. A stalled observer keeps the newest useful fact.
			 */
			const observers = [...current.event_observers.values()];
			for (const observer of observers) Queue.offerUnsafe(observer, event);

			const event_cursors = {
				...current.event_cursors,
				[event.stream_id]: event.sequence,
			};
			const last_journal_sequence = Math.max(
				current.last_journal_sequence,
				event.journal_sequence,
			);

			return [
				{
					_tag: "Applied",
					cursors: {
						event_cursors: record_to_cursors(event_cursors),
						last_journal_sequence,
					},
				},
				{ ...current, event_cursors, last_journal_sequence },
			];
		}).pipe(
			Effect.flatMap((outcome) => {
				switch (outcome._tag) {
					case "Applied":
						return Effect.succeed(outcome.cursors);
					case "Duplicate":
						return cursors;
					case "Gap":
						return Effect.fail(
							client_error(
								"stream_gap",
								"A durable event stream sequence contained a gap.",
								new Error("durable event cursor gap"),
								true,
							),
						);
				}
			}),
		);

	const apply_replay_complete = (
		envelope: Extract<OutboundControlEnvelope, { kind: "replay.complete" }>,
	) =>
		Ref.modify<SubscriptionState, boolean>(state, (current) => {
			const replay_cursors = cursors_to_record(envelope.payload.current_event_cursors);
			const cursor_regressed = Object.entries(current.event_cursors).some(
				([stream_id, sequence]) => (replay_cursors[stream_id] ?? 0) < sequence,
			);
			const journal_regressed =
				envelope.payload.journal_sequence < current.last_journal_sequence;

			if (cursor_regressed || journal_regressed) {
				return [false, current];
			}

			return [
				true,
				{
					...current,
					event_cursors: replay_cursors,
					last_journal_sequence: envelope.payload.journal_sequence,
				},
			];
		}).pipe(
			Effect.filterOrFail(
				(applied) => applied,
				() =>
					client_error(
						"stream_gap",
						"The replay boundary regressed behind the applied event cursor.",
						envelope,
						true,
					),
			),
			Effect.asVoid,
		);

	return {
		ApplyEvent: apply_event,
		ApplyReplayComplete: apply_replay_complete,
		Cursors: cursors,
		DropCursors: drop_cursors,
	};
});
