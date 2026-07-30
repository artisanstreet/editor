import { Effect } from "effect";
import { eq } from "drizzle-orm";

import { ConversationItem } from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import { ConversationItems } from "../../persistence/tables";
import type { ConversationObservationContext } from "./domain";
import { text } from "./domain";
import { Decode, DecodeJson, Emit, UpsertItem } from "./entities";

/** Appends a provider delta to its stable item while retaining one renderer entity. */
export const AppendText = (
	transaction: DatabaseClient,
	thread_id: string,
	item_id: string,
	turn_id: string,
	input: ConversationObservationContext,
	value: string,
	type: "assistant_message" | "reasoning_summary",
	source: { observed_at: string },
	phase?: "commentary" | "final" | "unspecified",
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item_id))
			.limit(1);
		const existing = rows.at(0);
		if (existing === undefined)
			return yield* UpsertItem(
				transaction,
				thread_id,
				{
					id: item_id,
					turn_id,
					created_at: input.occurred_at,
					updated_at: input.occurred_at,
					lifecycle: "streaming",
					references: [],
					source_refs: [{ reference: item_id, provider: "engine" }],
					...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
					run_id: input.run_id,
					type,
					text: text(value),
					...(type === "assistant_message" ? { phase: phase ?? "unspecified" } : {}),
				},
				source,
			);
		const prior = yield* DecodeJson(
			ConversationItem,
			existing.entity_json,
			"stored conversation item",
		);
		if (
			(prior.type !== "assistant_message" && prior.type !== "reasoning_summary") ||
			prior.lifecycle !== "streaming"
		)
			return prior;
		const delta = text(value).slice(0, Math.max(0, 4_096 - prior.text.length));
		const revision = prior.revision + 1;
		const entity = yield* Decode(
			ConversationItem,
			{
				...prior,
				text: prior.text + delta,
				revision,
				updated_at: input.occurred_at,
			},
			"appended conversation item",
		);
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, item_id));
		yield* Emit(transaction, thread_id, input.occurred_at, {
			type: "item_append",
			item_id,
			text: delta,
			revision,
		});
		return entity;
	});

/**
 * Settles a displayed reasoning summary. A missing item means reasoning display
 * was suppressed, so completion remains a no-op.
 */
export const CompleteReasoningSummary = (
	transaction: DatabaseClient,
	thread_id: string,
	item_id: string,
	occurred_at: string,
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item_id))
			.limit(1);
		const existing = rows.at(0);
		if (existing === undefined) return;
		const prior = yield* DecodeJson(
			ConversationItem,
			existing.entity_json,
			"stored conversation item",
		);
		if (
			prior.type !== "reasoning_summary" ||
			["completed", "failed", "cancelled"].includes(prior.lifecycle)
		)
			return prior;
		const revision = prior.revision + 1;
		const entity = yield* Decode(
			ConversationItem,
			{ ...prior, lifecycle: "completed", revision, updated_at: occurred_at },
			"completed reasoning summary",
		);
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, item_id));
		yield* Emit(transaction, thread_id, occurred_at, {
			type: "item_lifecycle",
			item_id,
			lifecycle: "completed",
			revision,
		});
		return entity;
	});
