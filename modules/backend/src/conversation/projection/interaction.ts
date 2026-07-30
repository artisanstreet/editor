import { Effect } from "effect";

import type { EngineObservation } from "@artisan/engines";

import type { DatabaseClient } from "../../persistence/database";
import type { ConversationObservationContext } from "./domain";
import { compaction_item_id, item_base, optional_text, text } from "./domain";
import { UpsertItem } from "./entities";

type InteractionObservation = Extract<
	EngineObservation,
	{ _tag: "approval" | "compaction" | "plan" | "question" | "retry" }
>;

export const ApplyInteractionObservation = (
	transaction: DatabaseClient,
	observation: InteractionObservation,
	input: ConversationObservationContext,
	turn_id: string,
) => {
	const source = { observed_at: input.occurred_at };

	switch (observation._tag) {
		case "plan":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`plan:${observation.observation_id}`,
						turn_id,
						input,
						"active",
						observation.observation_id,
					),
					type: "plan",
					state: "active",
					entries: observation.entries.map((entry) => ({
						id: entry.id,
						text: text(entry.text) || "Plan entry",
						state: entry.status === "in_progress" ? "active" : entry.status,
					})),
				},
				source,
			);
		case "approval":
			return Effect.gen(function* () {
				const approval_reason = optional_text(observation.request.reason);
				const approval_command =
					observation.request.kind === "command"
						? optional_text(observation.request.command)
						: undefined;
				const approval_cwd =
					observation.request.kind === "command"
						? optional_text(observation.request.cwd)
						: undefined;
				const request =
					observation.request.kind === "command"
						? {
								kind: "command" as const,
								...(approval_command === undefined
									? {}
									: { command: approval_command }),
								...(approval_cwd === undefined ? {} : { cwd: approval_cwd }),
								...(approval_reason === undefined
									? {}
									: { reason: approval_reason }),
							}
						: observation.request.kind === "file_change"
							? {
									kind: "file_change" as const,
									...(approval_reason === undefined
										? {}
										: { reason: approval_reason }),
								}
							: {
									kind: "action" as const,
									...(approval_reason === undefined
										? {}
										: { reason: approval_reason }),
								};
				return yield* UpsertItem(
					transaction,
					input.thread_id,
					{
						...item_base(
							`approval:${observation.approval_id}`,
							turn_id,
							input,
							observation.state === "requested" ? "waiting" : "completed",
							observation.observation_id,
						),
						type: "approval",
						interaction_id: observation.approval_id,
						prompt:
							approval_reason ??
							optional_text(observation.description) ??
							"Approval requested",
						request,
						requested_at: input.occurred_at,
						state:
							observation.state === "requested"
								? "requested"
								: observation.approved
									? "approved"
									: "rejected",
						...(observation.state === "resolved"
							? {
									resolution: observation.approved ? "Approved" : "Rejected",
									resolved_at: input.occurred_at,
								}
							: {}),
					},
					source,
				);
			});
		case "question":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`question:${observation.question_id}`,
						turn_id,
						input,
						observation.state === "requested" ? "waiting" : "completed",
						observation.observation_id,
					),
					type: "question",
					interaction_id: observation.question_id,
					prompt: text(observation.text) || "Question",
					requested_at: input.occurred_at,
					state: observation.state === "requested" ? "requested" : "answered",
					...(observation.answers
						? {
								resolution:
									text(observation.answers.flat().join("; ")) || "Answered",
								resolved_at: input.occurred_at,
							}
						: {}),
				},
				source,
			);
		case "compaction":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						compaction_item_id(
							input.run_id,
							observation.observation_id,
							observation.compaction_id,
						),
						turn_id,
						input,
						observation.state === "completed" ? "completed" : "active",
						observation.observation_id,
					),
					type: "compaction",
					state: observation.state,
					portability: "provider_bound",
					...(observation.summary ? { summary: text(observation.summary) } : {}),
				},
				source,
			);
		case "retry":
			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`error:${observation.observation_id}`,
						turn_id,
						input,
						observation.will_retry ? "waiting" : "failed",
						observation.observation_id,
					),
					type: "error",
					message: text(observation.message) || "Provider error",
					retry: observation.will_retry
						? { kind: "scheduled", after_ms: 0, attempt: 0, max_attempts: 0 }
						: { kind: "none" },
				},
				source,
			);
	}
};
