import { Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import type { DatabaseClient } from "../../persistence/database";
import type { ConversationObservationContext } from "./domain";
import { item_base, lifecycle, optional_text, text } from "./domain";
import { UpsertItem } from "./entities";

type ActivityObservation = Extract<
	EngineObservation,
	{
		_tag:
			| "file"
			| "native_action"
			| "process_diagnostic"
			| "protocol_diagnostic"
			| "search"
			| "terminal_activity"
			| "tool"
			| "usage";
	}
>;

export const ApplyActivityObservation = (
	transaction: DatabaseClient,
	observation: ActivityObservation,
	input: ConversationObservationContext,
	turn_id: string,
) => {
	const source = { observed_at: input.occurred_at };

	switch (observation._tag) {
		case "file":
			if (observation.action === "read")
				return UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`activity:${observation.observation_id}`,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						kind: "file",
						label: "Read file",
						status: "completed",
						detail: text(observation.path) || "Unknown file",
						type: "activity",
					},
					source,
				);
			return Effect.gen(function* () {
				const file_id = `file:${observation.observation_id}`;
				const change_set_id = `change-set:${observation.observation_id}`;
				yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							change_set_id,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						file_count: 1,
						file_ids: [file_id],
						state: "applied",
						summary: `Changed ${text(observation.path) || "file"}`,
						type: "change_set",
					},
					source,
				);
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							file_id,
							turn_id,
							input,
							"completed",
							observation.observation_id,
						),
						change_set_id,
						diff: { kind: "unavailable" },
						operation: observation.action,
						path: text(observation.path) || "Unknown file",
						type: "file_change",
					},
					source,
				);
			});
		case "terminal_activity":
		case "tool":
		case "search":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`activity:${observation.observation_id}`,
						turn_id,
						input,
						observation._tag === "tool" && observation.action === "failed"
							? "failed"
							: "active",
						observation.observation_id,
					),
					type: "activity",
					kind: observation._tag,
					label:
						observation._tag === "tool"
							? text(observation.tool_name) || "Tool"
							: observation._tag === "search"
								? "Search"
								: "Terminal",
					status:
						observation._tag === "tool"
							? lifecycle(observation.action)
							: lifecycle(observation.state),
					...(observation._tag === "tool" && observation.detail
						? { detail: text(observation.detail) }
						: {}),
				},
				source,
			);
		case "native_action":
		case "protocol_diagnostic":
		case "process_diagnostic":
		case "usage":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`native:${observation.observation_id}`,
						turn_id,
						input,
						"completed",
						observation.observation_id,
					),
					type: "native_event",
					summary:
						observation._tag === "native_action"
							? (optional_text(observation.detail) ??
								(text(observation.action) || "Native engine action"))
							: observation._tag === "usage"
								? "Usage update"
								: text(observation.message) || "Engine diagnostic",
				},
				source,
			);
	}
};
