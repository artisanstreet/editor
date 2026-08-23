import { Effect } from "effect";
import { and, asc, count, desc, eq, gt, gte, inArray, lt, sql } from "drizzle-orm";

import {
	ConversationPatch,
	ConversationSnapshot,
	conversation_query_maximum_turn_count,
	type ConversationQueryRange,
	type ConversationQueryWindow,
} from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import {
	ConversationItems,
	ConversationPatches,
	ConversationThreads,
	ConversationTurns,
} from "../../persistence/tables";
import { ConversationProjectionError } from "./domain";
import { Decode, DecodeJson } from "./entities";

/** Caps every replay query and its corresponding transport envelope. */
export const conversation_patch_replay_batch_size = 64;

/** Whole-thread navigation stays useful without shipping unbounded marker lists. */
const conversation_window_marker_limit = 512;

/** How one snapshot read is bounded; omitted entirely means the full thread. */
export interface ConversationReadBounds {
	readonly range?: ConversationQueryRange | undefined;
	readonly window?: ConversationQueryWindow | undefined;
}

const clamp_turn_count = (value: number) =>
	Math.min(Math.max(1, Math.trunc(value)), conversation_query_maximum_turn_count);

const ParseStoredEntities = (
	rows: ReadonlyArray<{ readonly entity_json: string }>,
	context: string,
) =>
	Effect.try({
		try: () => rows.map((row) => JSON.parse(row.entity_json)),
		catch: () => new ConversationProjectionError(`Invalid ${context}`),
	});

/** The newest matching turns, returned oldest-first for snapshot assembly. */
const ReadNewestTurns = (
	transaction: DatabaseClient,
	thread_id: string,
	maximum: number,
	before_turn_ordinal?: number,
	minimum_turn_ordinal?: number,
) =>
	transaction
		.select()
		.from(ConversationTurns)
		.where(
			and(
				eq(ConversationTurns.thread_id, thread_id),
				...(before_turn_ordinal === undefined
					? []
					: [lt(ConversationTurns.ordinal, before_turn_ordinal)]),
				...(minimum_turn_ordinal === undefined
					? []
					: [gte(ConversationTurns.ordinal, minimum_turn_ordinal)]),
			),
		)
		.orderBy(desc(ConversationTurns.ordinal))
		.limit(clamp_turn_count(maximum))
		.pipe(Effect.map((rows) => [...rows].reverse()));

/**
 * Every user message in the thread with its turn's ordinal, oldest-first.
 * A marker is the navigator's whole-thread anchor and the hydration target
 * that tells a windowed reader how far back an older range must descend.
 */
const ReadConversationMarkers = (transaction: DatabaseClient, thread_id: string) =>
	transaction
		.select({
			entity_json: ConversationItems.entity_json,
			ordinal: ConversationItems.ordinal,
			turn_ordinal: ConversationTurns.ordinal,
		})
		.from(ConversationItems)
		.innerJoin(ConversationTurns, eq(ConversationItems.turn_id, ConversationTurns.turn_id))
		.where(
			and(
				eq(ConversationItems.thread_id, thread_id),
				sql`json_extract(${ConversationItems.entity_json}, '$.type') = 'user_message'`,
			),
		)
		.orderBy(desc(ConversationItems.ordinal))
		.limit(conversation_window_marker_limit)
		.pipe(
			Effect.flatMap((rows) =>
				Effect.try({
					try: () =>
						[...rows].reverse().map((row) => {
							const entity: unknown = JSON.parse(row.entity_json);
							const text =
								typeof entity === "object" &&
								entity !== null &&
								"text" in entity &&
								typeof entity.text === "string"
									? entity.text
									: "";
							const id =
								typeof entity === "object" &&
								entity !== null &&
								"id" in entity &&
								typeof entity.id === "string"
									? entity.id
									: "";
							return {
								id,
								label: text.slice(0, 200),
								ordinal: row.ordinal,
								turn_ordinal: row.turn_ordinal,
							};
						}),
					catch: () =>
						new ConversationProjectionError("Invalid stored conversation markers"),
				}),
			),
		);

/**
 * Decodes a durable conversation snapshot. Without bounds it carries the whole
 * thread. A `window` bound carries only the newest turns plus whole-thread
 * markers; a `range` bound carries one older slice for client-side hydration
 * and no window metadata. Items always follow their turns, so a long-running
 * turn's late items are never separated from it.
 */
export const ReadConversationSnapshot = (
	transaction: DatabaseClient,
	thread_id: string,
	bounds?: ConversationReadBounds,
) =>
	Effect.gen(function* () {
		const thread_rows = yield* transaction
			.select()
			.from(ConversationThreads)
			.where(eq(ConversationThreads.thread_id, thread_id))
			.limit(1);
		const thread = thread_rows.at(0);
		if (thread === undefined) return;
		const range = bounds?.range;
		const window = range === undefined ? bounds?.window : undefined;
		const [turns, total_rows, markers] = yield* Effect.all(
			[
				range !== undefined
					? ReadNewestTurns(
							transaction,
							thread_id,
							range.maximum_turn_count,
							range.before_turn_ordinal,
							range.minimum_turn_ordinal,
						)
					: window !== undefined
						? ReadNewestTurns(transaction, thread_id, window.maximum_turn_count)
						: transaction
								.select()
								.from(ConversationTurns)
								.where(eq(ConversationTurns.thread_id, thread_id))
								.orderBy(asc(ConversationTurns.ordinal)),
				window === undefined
					? Effect.succeed(undefined)
					: transaction
							.select({ value: count() })
							.from(ConversationTurns)
							.where(eq(ConversationTurns.thread_id, thread_id))
							.pipe(Effect.map((rows) => rows.at(0)?.value ?? 0)),
				window === undefined
					? Effect.succeed(undefined)
					: ReadConversationMarkers(transaction, thread_id),
			],
			{ concurrency: "unbounded" },
		);
		const bounded = range !== undefined || window !== undefined;
		const items = yield* (bounded
			? turns.length === 0
				? Effect.succeed([])
				: transaction
						.select()
						.from(ConversationItems)
						.where(
							and(
								eq(ConversationItems.thread_id, thread_id),
								inArray(
									ConversationItems.turn_id,
									turns.map((turn) => turn.turn_id),
								),
							),
						)
						.orderBy(asc(ConversationItems.ordinal))
			: transaction
					.select()
					.from(ConversationItems)
					.where(eq(ConversationItems.thread_id, thread_id))
					.orderBy(asc(ConversationItems.ordinal)));
		/*
		 * Stored entities are validated when written and the final snapshot decode
		 * validates them once on read. This custom bulk JSON boundary is necessary
		 * because Effect has no bulk JSON parse that avoids per-row Effect/schema
		 * overhead while preserving typed corruption failures.
		 */
		const [decoded_turns, decoded_items] = yield* Effect.all([
			ParseStoredEntities(turns, "stored conversation turns"),
			ParseStoredEntities(items, "stored conversation items"),
		]);
		return yield* Decode(
			ConversationSnapshot,
			{
				conversation_id: `conversation:${thread_id}`,
				thread_id,
				schema_version: 1,
				journal_sequence: thread.journal_sequence,
				last_patch_sequence: thread.last_patch_sequence,
				updated_at: thread.updated_at,
				turns: decoded_turns,
				items: decoded_items,
				...(window === undefined || total_rows === undefined || markers === undefined
					? {}
					: {
							window: {
								markers: markers.filter((marker) => marker.id.length > 0),
								total_turn_count: total_rows,
							},
						}),
			},
			"conversation snapshot",
		);
	});

export const ReadConversationPatches = (
	transaction: DatabaseClient,
	thread_id: string,
	after_sequence: number,
	maximum = conversation_patch_replay_batch_size,
) =>
	transaction
		.select()
		.from(ConversationPatches)
		.where(
			and(
				eq(ConversationPatches.thread_id, thread_id),
				gt(ConversationPatches.sequence, after_sequence),
			),
		)
		.orderBy(asc(ConversationPatches.sequence))
		.limit(Math.min(Math.max(1, maximum), conversation_patch_replay_batch_size))
		.pipe(
			Effect.flatMap((rows) =>
				Effect.forEach(rows, (row) =>
					DecodeJson(ConversationPatch, row.patch_json, "stored conversation patch"),
				),
			),
		);
