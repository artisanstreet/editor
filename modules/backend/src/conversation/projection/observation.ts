import { Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import type { DatabaseClient } from "../../persistence/database";
import { ApplyActivityObservation } from "./activity";
import type { ConversationObservationContext } from "./domain";
import { item_base, lifecycle, text, turn_base } from "./domain";
import { Admit, EnsureThread, UpsertItem, UpsertTurn } from "./entities";
import { ApplyInteractionObservation } from "./interaction";
import { AppendText, CompleteReasoningSummary } from "./messages";

/** Applies one normalized engine observation in the caller's transaction. */
export const ApplyEngineObservation = (
	transaction: DatabaseClient,
	observation: EngineObservation,
	input: ConversationObservationContext,
) =>
	Effect.gen(function* () {
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
		yield* UpsertTurn(
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
						text: text(observation.message),
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
					input.occurred_at,
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
						started_at: input.occurred_at,
						status: lifecycle(observation.state),
						title: "Agent work",
						type: "work_session",
					},
					source,
				);
			case "run_state":
			case "run_terminal":
				return yield* UpsertTurn(
					transaction,
					input.thread_id,
					turn_base(turn_id, input, observation.state, observation.observation_id),
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
			case "usage":
				return yield* ApplyActivityObservation(transaction, observation, input, turn_id);
		}
	});
