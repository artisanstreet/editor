import { Effect } from "effect";

import type { EventEnvelope } from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import { body_text, item_base, lifecycle, source_refs, turn_base } from "./domain";
import { Admit, EnsureThread, UpsertItem, UpsertTurn } from "./entities";
import { MarkStreamingAssistantSteeringBoundaries } from "./messages";

/** Applies journal facts which are canonical user-visible conversation input. */
export const ApplyJournalEvent = (transaction: DatabaseClient, event: EventEnvelope) =>
	Effect.gen(function* () {
		yield* EnsureThread(transaction, event.thread_id, event.sent_at);
		const admitted = yield* Admit(
			transaction,
			`event:${event.message_id}`,
			event.thread_id,
			event.sent_at,
			event.journal_sequence,
		);
		if (!admitted) return;

		const payload = event.payload;
		const run_id = event.run_id ?? "journal";
		const context = {
			thread_id: event.thread_id,
			run_id,
			occurred_at: event.sent_at,
		};
		const source = {
			observed_at: event.sent_at,
			journal_sequence: event.journal_sequence,
		};
		const event_source_refs = (reference: string) =>
			source_refs(reference, {
				event_id: event.message_id,
				journal_sequence: event.journal_sequence,
			});

		if (payload.type === "usage.interruption.updated") {
			const interruption = payload.interruption;
			const item_lifecycle =
				interruption.state === "continued"
					? "completed"
					: interruption.state === "cancelled" || interruption.state === "failed"
						? interruption.state
						: "active";
			yield* UpsertItem(
				transaction,
				event.thread_id,
				{
					...item_base(
						`usage-interruption:${interruption.interruption_id}`,
						`run:${interruption.source_run_id}`,
						{
							agent_id: interruption.source_agent_id,
							occurred_at: event.sent_at,
							run_id: interruption.source_run_id,
							thread_id: event.thread_id,
						},
						item_lifecycle,
						event.message_id,
					),
					interruption,
					source_refs: event_source_refs(interruption.interruption_id),
					type: "usage_interruption",
				},
				source,
			);
			return;
		}

		if (payload.type === "thread.model_transition") {
			const transition_state = payload.state ?? "completed";
			yield* UpsertItem(
				transaction,
				event.thread_id,
				{
					/**
					 * Keyed on the run being handed to, not on the event: the start and
					 * the landing are two events describing one handoff, and the second
					 * has to complete the item the first opened rather than add a row.
					 */
					...item_base(
						`model-transition:${run_id}`,
						`run:${run_id}`,
						context,
						transition_state === "started" ? "active" : "completed",
						event.message_id,
					),
					state: transition_state,
					continuation: payload.continuation,
					source_engine_id: payload.source.engine_id,
					...(payload.source.model_id === undefined
						? {}
						: { source_model_id: payload.source.model_id }),
					target_engine_id: payload.target.engine_id,
					...(payload.target.model_id === undefined
						? {}
						: { target_model_id: payload.target.model_id }),
					type: "model_transition",
					source_refs: event_source_refs(event.message_id),
				},
				source,
			);
			return;
		}

		if (payload.type === "run.lifecycle" && event.run_id !== undefined) {
			const turn_id = `run:${event.run_id}`;
			const state = lifecycle(payload.state);
			yield* UpsertTurn(
				transaction,
				event.thread_id,
				{
					...turn_base(turn_id, context, state, event.message_id),
					source_refs: event_source_refs(event.message_id),
				},
				source,
			);
			yield* UpsertItem(
				transaction,
				event.thread_id,
				{
					...item_base(`work:${turn_id}`, turn_id, context, state, event.message_id),
					...(state === "completed" ||
					state === "failed" ||
					state === "interrupted" ||
					state === "cancelled"
						? { ended_at: event.sent_at }
						: {}),
					...(state === "failed" && payload.failure !== undefined
						? { failure: payload.failure }
						: {}),
					/**
					 * A run the host ended can be picked back up: native resume when
					 * the token survived, and a replay of the original message when it
					 * did not. Stated on every transition rather than only when true,
					 * so a resumed session clears the offer as it reopens.
					 */
					resumable: state === "interrupted",
					started_at: event.sent_at,
					status: state,
					title: "Agent work",
					type: "work_session",
					source_refs: event_source_refs(event.message_id),
				},
				source,
			);
			return;
		}

		if (payload.type !== "thread.message_queued" && payload.type !== "thread.message_steering")
			return;
		const turn_id = `turn:user:${payload.message_id}`;
		yield* UpsertTurn(
			transaction,
			event.thread_id,
			{
				...turn_base(turn_id, context, "completed", event.message_id),
				source_refs: event_source_refs(payload.message_id),
			},
			source,
		);
		const message = yield* UpsertItem(
			transaction,
			event.thread_id,
			{
				...item_base(
					`message:${payload.message_id}`,
					turn_id,
					context,
					"completed",
					event.message_id,
				),
				...(payload.attachments === undefined ? {} : { attachments: payload.attachments }),
				...(payload.content === undefined ? {} : { content: payload.content }),
				type: "user_message",
				text: body_text(payload.text) || "Message",
				source_refs: event_source_refs(payload.message_id),
			},
			source,
		);
		if (payload.type === "thread.message_steering" && event.run_id !== undefined) {
			yield* MarkStreamingAssistantSteeringBoundaries(
				transaction,
				event.thread_id,
				event.run_id,
				message.id,
				event.sent_at,
			);
		}
	});
