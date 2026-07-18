import { and, asc, desc, eq, gt, inArray, lt } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	JournalSequence,
	TranscriptEntry,
	type ThreadTranscriptQuery,
	type ThreadTranscriptSnapshot,
} from "@artisan/protocol";

import { Database } from "./database";
import { JournalEvents, ThreadTombstones, Threads } from "./schema";
import { JournalInvariantError } from "./journal-store";

export class TranscriptReadModelFailure extends Data.TaggedError("TranscriptReadModelFailure")<{
	readonly cause: unknown;
}> {}

export type TranscriptReadModelError = JournalInvariantError | TranscriptReadModelFailure;

export class TranscriptReadModel extends Context.Service<
	TranscriptReadModel,
	{
		readonly Read: (
			query: ThreadTranscriptQuery,
		) => Effect.Effect<ThreadTranscriptSnapshot, TranscriptReadModelError>;
	}
>()("Artisan/TranscriptReadModel") {}

const transcript_types = [
	"thread.message_queued",
	"thread.message_steering",
	"assistant.message_completed",
	"interaction.approval",
	"interaction.question",
	"intake.assessed",
	"intake.assumption_recorded",
] as const;

const watermark = (value: unknown) =>
	Schema.decodeUnknownEffect(JournalSequence)(value).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Transcript watermark is not a valid journal sequence",
				}),
		),
	);

const normalize = (error: unknown): TranscriptReadModelError =>
	error instanceof JournalInvariantError
		? error
		: new TranscriptReadModelFailure({ cause: error });

/** Reconstructs only allow-listed journal facts; raw provider frames never cross this boundary. */
export const TranscriptReadModelLive = Layer.effect(
	TranscriptReadModel,
	Effect.gen(function* () {
		const database = yield* Database;
		const Read = (query: ThreadTranscriptQuery) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const [erased] = yield* transaction
							.select({ thread_id: ThreadTombstones.thread_id })
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, query.thread_id))
							.limit(1);
						const [thread] = yield* transaction
							.select({ thread_id: Threads.thread_id })
							.from(Threads)
							.where(eq(Threads.thread_id, query.thread_id))
							.limit(1);
						const [latest] = yield* transaction
							.select({ sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const journal_sequence = latest ? yield* watermark(latest.sequence) : 0;
						if (erased)
							return { status: "erased" as const, journal_sequence, entries: [] };
						if (!thread)
							return {
								status: "unavailable" as const,
								journal_sequence,
								entries: [],
							};
						const limit = Math.min(query.limit ?? 200, 500);
						const condition = and(
							eq(JournalEvents.thread_id, query.thread_id),
							inArray(JournalEvents.event_type, transcript_types),
							...(query.after_journal_sequence === undefined
								? []
								: [gt(JournalEvents.sequence, query.after_journal_sequence)]),
							...(query.before_journal_sequence === undefined
								? []
								: [lt(JournalEvents.sequence, query.before_journal_sequence)]),
						);
						const descending = query.after_journal_sequence === undefined;
						const rows = yield* transaction
							.select()
							.from(JournalEvents)
							.where(condition)
							.orderBy(
								descending
									? desc(JournalEvents.sequence)
									: asc(JournalEvents.sequence),
							)
							.limit(limit);
						const decoded = yield* Effect.forEach(
							descending ? [...rows].reverse() : rows,
							(row) => {
								return Effect.try({
									try: () => JSON.parse(row.payload_json) as unknown,
									catch: () =>
										new JournalInvariantError({
											message: "Transcript payload contains invalid JSON",
										}),
								}).pipe(
									Effect.flatMap((payload) =>
										Schema.decodeUnknownEffect(TranscriptEntry)({
											event_id: row.event_id,
											journal_sequence: row.sequence,
											occurred_at: row.occurred_at,
											payload,
										}),
									),
									Effect.mapError(
										() =>
											new JournalInvariantError({
												message: "Transcript payload failed safe decoding",
											}),
									),
									Effect.map(Option.some),
								);
							},
						);
						const entries = decoded.filter(Option.isSome).map((entry) => entry.value);
						const oldest = entries[0]?.journal_sequence;
						return {
							status: "available" as const,
							journal_sequence,
							entries,
							...(rows.length === limit && oldest !== undefined
								? { next_before_journal_sequence: oldest }
								: {}),
						};
					}),
				)
				.pipe(Effect.mapError(normalize));
		return { Read };
	}),
);
