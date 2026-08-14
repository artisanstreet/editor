import { Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import type { DatabaseClient } from "../../persistence/database";
import { ApplyActivityObservation } from "./activity";
import type { ConversationObservationContext } from "./domain";
import { body_text, item_base, lifecycle, terminal_failure, turn_base } from "./domain";
import { Admit, EnsureThread, EnsureTurn, UpsertItem, UpsertTurn } from "./entities";
import { ApplyInteractionObservation, CancelPendingInteractions } from "./interaction";
import { AppendText, CompleteReasoningSummary, SettleStreamingBodies } from "./messages";

/** Applies one normalized engine observation in the caller's transaction. */
export const ApplyEngineObservation = (
	transaction: DatabaseClient,
	observation: EngineObservation,
	input: ConversationObservationContext,
) =>
	Effect.gen(function* () {
		/**
		 * Native child-agent lifecycle belongs to the orchestration graph, not the
		 * root conversation transcript. In particular, a child completion must not
		 * settle or otherwise manufacture the parent renderer turn.
		 */
		if (observation._tag === "subagent" || observation._tag === "subagent_transcript") return;
		if (
			observation._tag === "usage" ||
			(observation._tag === "terminal_activity" && observation.state === "output") ||
			(observation._tag === "tool" && observation.action === "progress") ||
			(observation._tag === "native_action" && observation.error_ref === undefined)
		)
			return;

		yield* EnsureThread(transaction, input.thread_id, input.occurred_at);
		const admitted = yield* Admit(
			transaction,
			`observation:${observation.observation_id}`,
			input.thread_id,
			input.occurred_at,
		);
		if (!admitted) return;

		/** One Artisan run is exactly one renderer turn. */
		const turn_id = `run:${input.run_id}`;
		const source = { observed_at: input.occurred_at };
		const is_body_delta =
			observation._tag === "agent_message_delta" ||
			observation._tag === "reasoning_summary_delta" ||
			(observation._tag === "reasoning_summary_completed" && observation.text !== undefined);
		yield* (is_body_delta ? EnsureTurn : UpsertTurn)(
			transaction,
			input.thread_id,
			turn_base(turn_id, input, "active", observation.observation_id),
			source,
		);

		switch (observation._tag) {
			case "agent_message_delta":
				return yield* AppendText(
					transaction,
					input.thread_id,
					observation.item_id,
					turn_id,
					input,
					observation.delta,
					"assistant_message",
					source,
					observation.phase,
				);
			case "agent_message_completed":
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							observation.item_id,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						type: "assistant_message",
						text: body_text(observation.message),
						phase: observation.phase,
					},
					source,
				);
			case "reasoning_summary_delta":
				return yield* AppendText(
					transaction,
					input.thread_id,
					observation.item_id,
					turn_id,
					input,
					observation.delta,
					"reasoning_summary",
					source,
				);
			case "reasoning_summary_completed":
				return yield* CompleteReasoningSummary(
					transaction,
					input.thread_id,
					observation.item_id,
					turn_id,
					input,
					observation.observation_id,
					observation.text,
				);
			case "turn_state":
				yield* UpsertTurn(
					transaction,
					input.thread_id,
					turn_base(turn_id, input, observation.state, observation.observation_id),
					source,
				);
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`work:${turn_id}`,
							turn_id,
							input,
							observation.state,
							observation.observation_id,
						),
						...(observation.state === "completed" ||
						observation.state === "cancelled" ||
						observation.state === "failed"
							? { ended_at: input.occurred_at }
							: {}),
						...(observation.state === "started" || observation.state === "waiting"
							? { responded_at: input.occurred_at }
							: {}),
						started_at: input.occurred_at,
						status: lifecycle(observation.state),
						title: "Agent work",
						type: "work_session",
					},
					source,
				);
			case "run_state":
				return yield* UpsertTurn(
					transaction,
					input.thread_id,
					turn_base(turn_id, input, observation.state, observation.observation_id),
					source,
				);
			case "run_terminal":
				yield* CancelPendingInteractions(
					transaction,
					input,
					turn_id,
					observation.observation_id,
				);
				yield* SettleStreamingBodies(
					transaction,
					input.thread_id,
					turn_id,
					observation.state,
					input.occurred_at,
				);
				yield* UpsertTurn(
					transaction,
					input.thread_id,
					turn_base(turn_id, input, observation.state, observation.observation_id),
					source,
				);
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`work:${turn_id}`,
							turn_id,
							input,
							observation.state,
							observation.observation_id,
						),
						...(observation.state === "completed" ||
						observation.state === "cancelled" ||
						observation.state === "failed"
							? { ended_at: input.occurred_at }
							: {}),
						...(observation.state === "failed"
							? { failure: terminal_failure(observation.error_ref) }
							: {}),
						started_at: input.occurred_at,
						status: lifecycle(observation.state),
						title: "Agent work",
						type: "work_session",
					},
					source,
				);
			case "approval":
			case "compaction":
			case "plan":
			case "question":
			case "retry":
				return yield* ApplyInteractionObservation(transaction, observation, input, turn_id);
			case "file":
			case "native_action":
			case "process_diagnostic":
			case "protocol_diagnostic":
			case "search":
			case "terminal_activity":
			case "tool":
				return yield* ApplyActivityObservation(transaction, observation, input, turn_id);
		}
	});
