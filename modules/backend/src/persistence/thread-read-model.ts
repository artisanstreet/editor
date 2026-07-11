import { asc, desc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import { JournalSequence, ThreadListItem } from "@artisan/protocol";

import { Database } from "./database";
import { JournalEvents, Threads } from "./schema";
import { JournalInvariantError } from "./journal-store";

export class ThreadReadModelFailure extends Data.TaggedError("ThreadReadModelFailure")<{
	readonly cause: unknown;
}> {}

export type ThreadReadModelError = JournalInvariantError | ThreadReadModelFailure;

export interface ThreadSnapshot {
	readonly journal_sequence: number;
	readonly threads: ReadonlyArray<ThreadListItem>;
}

export class ThreadReadModel extends Context.Service<
	ThreadReadModel,
	{
		readonly Lookup: (
			thread_id: string,
		) => Effect.Effect<Option.Option<ThreadListItem>, ThreadReadModelError>;
		readonly Snapshot: () => Effect.Effect<ThreadSnapshot, ThreadReadModelError>;
	}
>()("Artisan/ThreadReadModel") {}

const DecodeJournalSequence = (value: unknown) =>
	Schema.decodeUnknownEffect(JournalSequence)(value).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Thread snapshot watermark is not a valid journal sequence",
				}),
		),
	);

const DecodePersistedJournalSequence = (value: unknown) =>
	DecodeJournalSequence(value).pipe(
		Effect.flatMap((sequence) => {
			if (sequence === 0) {
				return new JournalInvariantError({
					message: "Thread snapshot watermark must be greater than zero",
				});
			}

			return Effect.succeed(sequence);
		}),
	);

const DecodeThreadListItem = (value: unknown) =>
	Schema.decodeUnknownEffect(ThreadListItem, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Thread projection row does not match the protocol schema",
				}),
		),
	);

const DecodePersistedThread = (thread: typeof Threads.$inferSelect) => {
	const { archived_at, current_goal, rename_suggestion, ...required } = thread;

	return DecodeThreadListItem({
		...required,
		...(archived_at === null ? {} : { archived_at }),
		...(current_goal === null ? {} : { current_goal }),
		...(rename_suggestion === null ? {} : { rename_suggestion }),
	});
};

function normalize_thread_read_model_error(error: unknown): ThreadReadModelError {
	if (error instanceof JournalInvariantError) {
		return error;
	}

	return new ThreadReadModelFailure({ cause: error });
}

export const ThreadReadModelLive = Layer.effect(
	ThreadReadModel,
	Effect.gen(function* () {
		const database = yield* Database;
		const Lookup = (thread_id: string) =>
			database.client
				.select()
				.from(Threads)
				.where(eq(Threads.thread_id, thread_id))
				.limit(1)
				.pipe(
					Effect.flatMap(([thread]) =>
						thread
							? DecodePersistedThread(thread).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none()),
					),
					Effect.mapError(normalize_thread_read_model_error),
				);

		const Snapshot = () =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const thread_rows = yield* transaction
							.select()
							.from(Threads)
							.orderBy(asc(Threads.created_at), asc(Threads.thread_id));
						const [watermark] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const journal_sequence = watermark
							? yield* DecodePersistedJournalSequence(watermark.journal_sequence)
							: 0;
						const threads = yield* Effect.forEach(thread_rows, DecodePersistedThread);

						return { journal_sequence, threads };
					}),
				)
				.pipe(Effect.mapError(normalize_thread_read_model_error));

		return { Lookup, Snapshot };
	}),
);
