import { Effect } from "effect";
import { and, asc, eq, gt } from "drizzle-orm";

import { ConversationPatch, ConversationSnapshot } from "@artisan/protocol";

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

const ParseStoredEntities = (
	rows: ReadonlyArray<{ readonly entity_json: string }>,
	context: string,
) =>
	Effect.try({
		try: () => rows.map((row) => JSON.parse(row.entity_json)),
		catch: () => new ConversationProjectionError(`Invalid ${context}`),
	});

/** Decodes a complete durable conversation snapshot. */
export const ReadConversationSnapshot = (transaction: DatabaseClient, thread_id: string) =>
	Effect.gen(function* () {
		const thread_rows = yield* transaction
			.select()
			.from(ConversationThreads)
			.where(eq(ConversationThreads.thread_id, thread_id))
			.limit(1);
		const thread = thread_rows.at(0);
		if (thread === undefined) return;
		const [turns, items] = yield* Effect.all(
			[
				transaction
					.select()
					.from(ConversationTurns)
					.where(eq(ConversationTurns.thread_id, thread_id))
					.orderBy(asc(ConversationTurns.ordinal)),
				transaction
					.select()
					.from(ConversationItems)
					.where(eq(ConversationItems.thread_id, thread_id))
					.orderBy(asc(ConversationItems.ordinal)),
			],
			{ concurrency: "unbounded" },
		);
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
