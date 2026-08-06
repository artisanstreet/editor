import { Effect, Schema } from "effect";
import { eq } from "drizzle-orm";

import { ConversationItem, conversation_body_text_limit } from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import { ConversationItems } from "../../persistence/tables";
import type { ConversationObservationContext } from "./domain";
import { body_text, ConversationProjectionError } from "./domain";
import { Decode, DecodeJson, Emit, UpsertItem } from "./entities";

/** Stored entities stay opaque on the hot path; only consumed fields are decoded. */
const StoredEntityJson = Schema.Record(Schema.String, Schema.Unknown);

/** The only fields the append hot path reads from a streaming item. */
const StreamingBodyFields = Schema.Struct({
	lifecycle: Schema.String,
	revision: Schema.Number,
	text: Schema.String,
	type: Schema.String,
});

const DecodeStreamingBodyFields = (stored: Record<string, unknown>) =>
	Schema.decodeUnknownEffect(StreamingBodyFields)(stored).pipe(
		Effect.mapError(() => new ConversationProjectionError("Invalid stored conversation item")),
	);

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
					text: body_text(value),
					...(type === "assistant_message" ? { phase: phase ?? "unspecified" } : {}),
				},
				source,
			);
		/**
		 * The stored entity was schema-validated when it was written, so the
		 * per-delta path re-validates only the fields the append consumes and
		 * carries the rest opaquely. The item is fully re-decoded once, at
		 * completion, instead of twice per streamed delta.
		 */
		const stored = yield* DecodeJson(
			StoredEntityJson,
			existing.entity_json,
			"stored conversation item",
		);
		const fields = yield* DecodeStreamingBodyFields(stored);
		if (
			(fields.type !== "assistant_message" && fields.type !== "reasoning_summary") ||
			fields.lifecycle !== "streaming"
		)
			return;
		const delta = body_text(value).slice(
			0,
			Math.max(0, conversation_body_text_limit - fields.text.length),
		);
		const revision = fields.revision + 1;
		yield* transaction
			.update(ConversationItems)
			.set({
				entity_json: JSON.stringify({
					...stored,
					revision,
					text: fields.text + delta,
					updated_at: input.occurred_at,
				}),
			})
			.where(eq(ConversationItems.item_id, item_id));
		yield* Emit(transaction, thread_id, input.occurred_at, {
			type: "item_append",
			item_id,
			text: delta,
			revision,
		});
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
