import { Effect } from "effect";
import { and, asc, eq, gt } from "drizzle-orm";

import {
	ConversationItem,
	ConversationPatch,
	ConversationSnapshot,
	ConversationTurn,
} from "@artisan/protocol";

import type { DatabaseClient } from "../../persistence/database";
import {
	ConversationItems,
	ConversationPatches,
	ConversationThreads,
	ConversationTurns,
} from "../../persistence/tables";
import { Decode, DecodeJson } from "./entities";

/** Caps every replay query and its corresponding transport envelope. */
export const conversation_patch_replay_batch_size = 64;

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
		const turns = yield* transaction
			.select()
			.from(ConversationTurns)
			.where(eq(ConversationTurns.thread_id, thread_id))
			.orderBy(asc(ConversationTurns.ordinal));
		const items = yield* transaction
			.select()
			.from(ConversationItems)
			.where(eq(ConversationItems.thread_id, thread_id))
			.orderBy(asc(ConversationItems.ordinal));
		return yield* Decode(
			ConversationSnapshot,
			{
				conversation_id: `conversation:${thread_id}`,
				thread_id,
				schema_version: 1,
				journal_sequence: thread.journal_sequence,
				last_patch_sequence: thread.last_patch_sequence,
				updated_at: thread.updated_at,
				turns: yield* Effect.forEach(turns, (row) =>
					DecodeJson(ConversationTurn, row.entity_json, "stored conversation turn"),
				),
				items: yield* Effect.forEach(items, (row) =>
					DecodeJson(ConversationItem, row.entity_json, "stored conversation item"),
				),
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
