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
				/**
				 * A mutation also earns a trace row, alongside the change items it
				 * has always produced. Those change items are the turn's summary and
				 * the renderer holds them back until the turn settles — correct for
				 * a summary, but it made every edit invisible for the length of the
				 * run: the agent narrated "writing the store first" over a trace
				 * showing nothing, which reads as a dropped call. The row is what
				 * exists now; the card remains what the turn amounted to.
				 */
				yield* UpsertItem(
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
						kind: observation.action === "deleted" ? "file_delete" : "file_edit",
						label:
							observation.action === "created"
								? "Created file"
								: observation.action === "deleted"
									? "Deleted file"
									: "Edited file",
						status: "completed",
						detail: text(observation.path) || "Unknown file",
						type: "activity",
					},
					source,
				);
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
						/**
						 * Counted only when the engine reported both sides of its own
						 * patch. Absent counts stay unavailable rather than rendering
						 * as a confident zero.
						 */
						diff:
							observation.lines_added === undefined ||
							observation.lines_deleted === undefined
								? { kind: "unavailable" }
								: {
										additions: observation.lines_added,
										deletions: observation.lines_deleted,
										kind: "known",
									},
						operation: observation.action,
						path: text(observation.path) || "Unknown file",
						type: "file_change",
					},
					source,
				);
			});
		case "terminal_activity":
		case "tool":
		case "search": {
			/**
			 * Keyed on the provider's own id for the work rather than on the frame.
			 * Start, output, and completion arrive as separate observations, so
			 * keying on the frame turns one command into a row per output chunk —
			 * rows that carry nothing to name them and never reach a terminal
			 * status, which leaves the group they sit in shimmering as though it
			 * were still working. An engine that discloses no id for a search
			 * falls back to the frame's and keeps that flaw.
			 */
			const activity_key =
				observation._tag === "terminal_activity"
					? observation.activity_id
					: observation._tag === "tool"
						? observation.tool_id
						: (observation.search_id ?? observation.observation_id);
			/**
			 * What the row did, in its own words: the command a terminal ran, the
			 * query a search asked, the arguments a tool was given. Without it every
			 * row of a kind renders as the same word. It rides the frame that starts
			 * the work, and the output and completion frames that follow merge onto
			 * the row that frame already named. A search that discloses no query
			 * keeps its label rather than showing an empty one.
			 */
			const activity_detail =
				observation._tag === "terminal_activity"
					? optional_text(observation.command)
					: observation._tag === "search"
						? optional_text(observation.query)
						: optional_text(observation.detail);

			return UpsertItem(
				transaction,
				input.thread_id,
				{
					...item_base(
						`activity:${activity_key}`,
						turn_id,
						input,
						observation._tag === "tool" && observation.action === "failed"
							? "failed"
							: "active",
						observation.observation_id,
					),
					type: "activity",
					/**
					 * A search names where it looked: the bare kind reads as a web
					 * search downstream, so a workspace-scoped one must say so or a
					 * grep chain counts itself as web searches.
					 */
					kind:
						observation._tag === "search" && observation.scope === "workspace"
							? "workspace_search"
							: observation._tag,
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
					...(activity_detail === undefined ? {} : { detail: activity_detail }),
				},
				source,
			);
		}
		case "native_action":
		case "protocol_diagnostic":
		case "process_diagnostic":
		case "usage":
			/**
			 * A frame the adapter could not read is not something the run did, so
			 * it earns no row. Raw provenance already holds the frame verbatim for
			 * anyone debugging the adapter; a transcript row only teaches a reader
			 * to distrust a run that was fine.
			 */
			if (observation._tag === "native_action" && observation.diagnostic === true) {
				return Effect.void;
			}
			/**
			 * A classified failure travels in Artisan custody: the adapter minted
			 * an `AE-*` code at the boundary, and the item carries it so the
			 * renderer resolves identity from the error catalog rather than from
			 * whatever prose the provider produced.
			 */
			const error_ref =
				observation._tag === "native_action" ||
				observation._tag === "process_diagnostic"
					? observation.error_ref
					: undefined;
			const error_detail = optional_text(error_ref?.detail);
			const error_provider_code = optional_text(error_ref?.provider_code);
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
					...(error_ref === undefined
						? {}
						: {
								error: {
									code: error_ref.artisan_code,
									...(error_detail === undefined ? {} : { detail: error_detail }),
									...(error_provider_code === undefined
										? {}
										: { provider_code: error_provider_code }),
									...(error_ref.resets_at === undefined
										? {}
										: { resets_at: error_ref.resets_at }),
								},
							}),
					/**
					 * Diagnostics keep the level the engine reported; usage reports and
					 * native actions are quiet provenance — unless the action carries a
					 * classified failure, which is exactly the thing a reader must see.
					 */
					severity:
						observation._tag === "protocol_diagnostic" ||
						observation._tag === "process_diagnostic"
							? observation.level
							: error_ref === undefined
								? "info"
								: "error",
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
