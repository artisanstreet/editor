import { Context, Data, Effect, Layer, Option, Schema } from "effect";
import { and, desc, eq } from "drizzle-orm";

import { ConversationSubscriptionCursor, type ThreadListItem } from "@artisan/protocol";

import type {
	ConversationPatch,
	ConversationSnapshot,
	MessageImageAttachment,
	MessageImageAttachmentQuery,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	JournalCommands,
	JournalEvents,
	MessageImageAttachments,
	ThreadTombstones,
	Threads,
	ConversationThreads,
} from "../persistence/tables";
import { ReadConversationPatches, ReadConversationSnapshot } from "./projection-api";

export class ConversationReadModelFailure extends Data.TaggedError("ConversationReadModelFailure")<{
	readonly cause: unknown;
}> {}

export type ConversationAvailability =
	| { readonly status: "available"; readonly snapshot: ConversationSnapshot }
	| { readonly status: "erased"; readonly journal_sequence: number }
	| { readonly status: "unavailable"; readonly journal_sequence: number };

export type ConversationCursorAvailability =
	| {
			readonly status: "available";
			readonly conversation_id: string;
			readonly last_patch_sequence: number;
	  }
	| { readonly status: "erased" | "unavailable" };

export class ConversationReadModel extends Context.Service<
	ConversationReadModel,
	{
		readonly ReadSnapshot: (
			thread_id: string,
		) => Effect.Effect<ConversationAvailability, ConversationReadModelFailure>;
		/**
		 * Fast path for `thread.open` after ThreadReadModel.Lookup resolved the
		 * thread. None means the conversation projection has not been created yet.
		 */
		readonly ReadOpenSnapshot: (
			thread: ThreadListItem,
		) => Effect.Effect<Option.Option<ConversationSnapshot>, ConversationReadModelFailure>;
		readonly ReadPatches: (
			thread_id: string,
			after_sequence: number,
		) => Effect.Effect<ReadonlyArray<ConversationPatch>, ConversationReadModelFailure>;
		readonly ReadCursor: (
			thread_id: string,
		) => Effect.Effect<ConversationCursorAvailability, ConversationReadModelFailure>;
		readonly ReadImageAttachment: (
			query: MessageImageAttachmentQuery,
		) => Effect.Effect<Option.Option<MessageImageAttachment>, ConversationReadModelFailure>;
	}
>()("Artisan/ConversationReadModel") {}

const watermark = (transaction: any) =>
	transaction
		.select({ sequence: JournalEvents.sequence })
		.from(JournalEvents)
		.orderBy(desc(JournalEvents.sequence))
		.limit(1)
		.pipe(Effect.map(([row]: ReadonlyArray<{ sequence: number }>) => row?.sequence ?? 0));

/** Reads renderer-safe conversation projections at a single SQLite transaction boundary. */
export const ConversationReadModelLive = Layer.effect(
	ConversationReadModel,
	Effect.gen(function* () {
		const database = yield* Database;
		const ReadSnapshot = (
			thread_id: string,
		): Effect.Effect<ConversationAvailability, ConversationReadModelFailure> =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const journal_sequence = yield* watermark(transaction);
						const [erased] = yield* transaction
							.select()
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, thread_id))
							.limit(1);
						if (erased) return { status: "erased" as const, journal_sequence };
						const [thread] = yield* transaction
							.select()
							.from(Threads)
							.where(eq(Threads.thread_id, thread_id))
							.limit(1);
						if (!thread) return { status: "unavailable" as const, journal_sequence };
						const snapshot = yield* ReadConversationSnapshot(transaction, thread_id);
						if (!snapshot)
							return {
								status: "available" as const,
								snapshot: {
									conversation_id: `conversation:${thread_id}`,
									items: [],
									journal_sequence,
									last_patch_sequence: 0,
									schema_version: 1 as const,
									thread_id,
									turns: [],
									updated_at: thread.last_activity_at,
								},
							};
						return { status: "available" as const, snapshot };
					}),
				)
				.pipe(
					Effect.mapError((cause) => new ConversationReadModelFailure({ cause })),
				) as any;
		const ReadOpenSnapshot = (
			thread: ThreadListItem,
		): Effect.Effect<Option.Option<ConversationSnapshot>, ConversationReadModelFailure> =>
			database.client
				.transaction((transaction) =>
					ReadConversationSnapshot(transaction, thread.thread_id).pipe(
						Effect.map((snapshot) =>
							snapshot === undefined ? Option.none() : Option.some(snapshot),
						),
					),
				)
				.pipe(
					Effect.mapError((cause) => new ConversationReadModelFailure({ cause })),
				) as any;
		const ReadPatches = (
			thread_id: string,
			after_sequence: number,
		): Effect.Effect<ReadonlyArray<ConversationPatch>, ConversationReadModelFailure> =>
			database.client
				.transaction((transaction) =>
					ReadConversationPatches(transaction, thread_id, after_sequence),
				)
				.pipe(
					Effect.mapError((cause) => new ConversationReadModelFailure({ cause })),
				) as any;
		const ReadCursor = (
			thread_id: string,
		): Effect.Effect<ConversationCursorAvailability, ConversationReadModelFailure> =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [erased] = yield* transaction
							.select({ thread_id: ThreadTombstones.thread_id })
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, thread_id))
							.limit(1);
						if (erased) return { status: "erased" as const };
						const [head] = yield* transaction
							.select({
								last_patch_sequence: ConversationThreads.last_patch_sequence,
							})
							.from(Threads)
							.leftJoin(
								ConversationThreads,
								eq(ConversationThreads.thread_id, Threads.thread_id),
							)
							.where(eq(Threads.thread_id, thread_id))
							.limit(1);
						if (!head) return { status: "unavailable" as const };
						const cursor = yield* Schema.decodeUnknownEffect(
							ConversationSubscriptionCursor,
						)({
							conversation_id: `conversation:${thread_id}`,
							last_patch_sequence: head.last_patch_sequence ?? 0,
						});
						return { ...cursor, status: "available" as const };
					}),
				)
				.pipe(
					Effect.mapError((cause) => new ConversationReadModelFailure({ cause })),
				) as any;
		const ReadImageAttachment = (
			query: MessageImageAttachmentQuery,
		): Effect.Effect<Option.Option<MessageImageAttachment>, ConversationReadModelFailure> =>
			database.client
				.transaction((transaction) =>
					transaction
						.select({
							attachment_id: MessageImageAttachments.attachment_id,
							content: MessageImageAttachments.content,
							media_type: MessageImageAttachments.media_type,
							name: MessageImageAttachments.name,
							size_bytes: MessageImageAttachments.size_bytes,
						})
						.from(MessageImageAttachments)
						.innerJoin(
							JournalCommands,
							eq(MessageImageAttachments.message_id, JournalCommands.message_id),
						)
						.where(
							and(
								eq(MessageImageAttachments.attachment_id, query.attachment_id),
								eq(JournalCommands.thread_id, query.thread_id),
							),
						)
						.limit(1)
						.pipe(
							Effect.map(([row]) =>
								row === undefined
									? Option.none<MessageImageAttachment>()
									: Option.some({
											bytes: new Uint8Array(row.content),
											id: row.attachment_id,
											media_type:
												row.media_type as MessageImageAttachment["media_type"],
											name: row.name,
											size_bytes: row.size_bytes,
										}),
							),
						),
				)
				.pipe(Effect.mapError((cause) => new ConversationReadModelFailure({ cause })));
		return { ReadCursor, ReadImageAttachment, ReadOpenSnapshot, ReadPatches, ReadSnapshot };
	}),
);
