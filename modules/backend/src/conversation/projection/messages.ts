import { Effect, Schema } from "effect";
import { and, eq } from "drizzle-orm";

import {
	ConversationItem,
	conversation_body_text_limit,
	conversation_maximum_steering_fragment_boundaries,
} from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import { ConversationItems } from "../../persistence/tables";
import type { ConversationObservationContext } from "./domain";
import { body_text, ConversationProjectionError, item_base } from "./domain";
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

/**
 * Records the exact streaming cut occupied when an acknowledged steer enters
 * the canonical projection. Equal offsets replace their older anchor so two
 * steers without a delta between them do not manufacture an empty fragment.
 */
export const MarkStreamingAssistantSteeringBoundaries = (
	transaction: DatabaseClient,
	thread_id: string,
	run_id: string,
	after_item_id: string,
	occurred_at: string,
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(
				and(
					eq(ConversationItems.thread_id, thread_id),
					eq(ConversationItems.turn_id, `run:${run_id}`),
				),
			);
		yield* Effect.forEach(rows, (row) =>
			Effect.gen(function* () {
				const prior = yield* DecodeJson(
					ConversationItem,
					row.entity_json,
					"stored conversation item",
				);
				if (
					(prior.type !== "assistant_message" && prior.type !== "reasoning_summary") ||
					prior.run_id !== run_id ||
					prior.lifecycle !== "streaming" ||
					(prior.type === "assistant_message" && prior.phase === "commentary")
				)
					return;
				const text_offset = prior.text.length;
				const prior_boundaries = prior.steering_fragment_boundaries ?? [];
				const boundaries = [
					...prior_boundaries.filter((boundary) => boundary.text_offset !== text_offset),
					{ after_item_id, text_offset },
				]
					.sort((left, right) => left.text_offset - right.text_offset)
					.slice(-conversation_maximum_steering_fragment_boundaries);
				if (
					prior_boundaries.length === boundaries.length &&
					prior_boundaries.every(
						(boundary, index) =>
							boundary.after_item_id === boundaries[index]?.after_item_id &&
							boundary.text_offset === boundaries[index]?.text_offset,
					)
				)
					return;
				const revision = prior.revision + 1;
				const entity = yield* Decode(
					ConversationItem,
					{
						...prior,
						revision,
						steering_fragment_boundaries: boundaries,
						updated_at: occurred_at,
					},
					"steering-fragment conversation item",
				);
				yield* transaction
					.update(ConversationItems)
					.set({ entity_json: JSON.stringify(entity) })
					.where(eq(ConversationItems.item_id, entity.id));
				yield* Emit(transaction, thread_id, occurred_at, {
					type: "item_upsert",
					item: entity,
				});
			}),
		);
	});

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
 * Records how much reasoning a phase has done, for an engine that counts it but
 * will not publish it.
 *
 * Kept apart from {@link AppendText} because nothing is being appended: the
 * count is cumulative and replaces, where text accumulates. It opens the same
 * item the deltas would have so a turn whose reasoning is encrypted still has
 * one row saying the model is working, and it is a full re-decode rather than
 * the streaming fast path because these frames arrive a handful of times per
 * turn rather than per token.
 */
export const RecordThinkingProgress = (
	transaction: DatabaseClient,
	thread_id: string,
	item_id: string,
	turn_id: string,
	input: ConversationObservationContext,
	thinking_tokens: number,
	source: { observed_at: string },
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
					type: "reasoning_summary",
					text: "",
					thinking_tokens,
				},
				source,
			);
		const prior = yield* DecodeJson(
			ConversationItem,
			existing.entity_json,
			"stored conversation item",
		);
		/** A settled phase keeps its final count; only a live one still climbs. */
		if (
			prior.type !== "reasoning_summary" ||
			prior.lifecycle !== "streaming" ||
			prior.thinking_tokens === thinking_tokens
		)
			return;
		const revision = prior.revision + 1;
		const entity = yield* Decode(
			ConversationItem,
			{ ...prior, revision, thinking_tokens, updated_at: input.occurred_at },
			"thinking-progress conversation item",
		);
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, entity.id));
		yield* Emit(transaction, thread_id, input.occurred_at, {
			type: "item_upsert",
			item: entity,
		});
	});

/**
 * Settles a displayed reasoning summary. An authoritative final public summary
 * replaces streamed deltas and can create a completed item when no delta arrived.
 * A completion without text remains a no-op when display was suppressed.
 */
export const CompleteReasoningSummary = (
	transaction: DatabaseClient,
	thread_id: string,
	item_id: string,
	turn_id: string,
	input: ConversationObservationContext,
	reference: string,
	text?: string,
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.item_id, item_id))
			.limit(1);
		const existing = rows.at(0);
		if (existing === undefined) {
			if (text === undefined) return;
			return yield* UpsertItem(
				transaction,
				thread_id,
				{
					...item_base(item_id, turn_id, input, "completed", reference),
					type: "reasoning_summary",
					text: body_text(text),
				},
				{ observed_at: input.occurred_at },
			);
		}
		const prior = yield* DecodeJson(
			ConversationItem,
			existing.entity_json,
			"stored conversation item",
		);
		if (prior.type !== "reasoning_summary") return prior;
		const final_text = text === undefined ? undefined : body_text(text);
		const already_terminal = ["completed", "failed", "cancelled"].includes(prior.lifecycle);
		if (already_terminal && (final_text === undefined || prior.text === final_text))
			return prior;
		const revision = prior.revision + 1;
		const entity = yield* Decode(
			ConversationItem,
			{
				...prior,
				...(final_text === undefined ? {} : { text: final_text }),
				...(already_terminal ? {} : { lifecycle: "completed" }),
				revision,
				updated_at: input.occurred_at,
			},
			"completed reasoning summary",
		);
		yield* transaction
			.update(ConversationItems)
			.set({ entity_json: JSON.stringify(entity) })
			.where(eq(ConversationItems.item_id, item_id));
		yield* Emit(
			transaction,
			thread_id,
			input.occurred_at,
			final_text === undefined
				? { type: "item_lifecycle", item_id, lifecycle: "completed", revision }
				: { type: "item_upsert", item: entity },
		);
		return entity;
	});

/**
 * A provider may terminate without emitting its item-level completion frame.
 * Settle every streamed text body owned by the run's renderer turn so retained
 * trailing text can become visible and the client never waits for another delta.
 */
export const SettleStreamingBodies = (
	transaction: DatabaseClient,
	thread_id: string,
	turn_id: string,
	terminal_state: "completed" | "cancelled" | "failed" | "interrupted" | "closed",
	occurred_at: string,
) =>
	Effect.gen(function* () {
		const rows = yield* transaction
			.select()
			.from(ConversationItems)
			.where(
				and(
					eq(ConversationItems.thread_id, thread_id),
					eq(ConversationItems.turn_id, turn_id),
				),
			);
		const terminal_lifecycle = terminal_state === "closed" ? "completed" : terminal_state;

		yield* Effect.forEach(rows, (row) =>
			Effect.gen(function* () {
				const prior = yield* DecodeJson(
					ConversationItem,
					row.entity_json,
					"stored conversation item",
				);
				if (
					(prior.type !== "assistant_message" && prior.type !== "reasoning_summary") ||
					prior.lifecycle !== "streaming"
				)
					return;

				const revision = prior.revision + 1;
				const entity = yield* Decode(
					ConversationItem,
					{
						...prior,
						lifecycle: terminal_lifecycle,
						revision,
						updated_at: occurred_at,
					},
					"settled streaming conversation item",
				);
				yield* transaction
					.update(ConversationItems)
					.set({ entity_json: JSON.stringify(entity) })
					.where(eq(ConversationItems.item_id, entity.id));
				yield* Emit(transaction, thread_id, occurred_at, {
					type: "item_lifecycle",
					item_id: entity.id,
					lifecycle: terminal_lifecycle,
					revision,
				});
			}),
		);
	});
