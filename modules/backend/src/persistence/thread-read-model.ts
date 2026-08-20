import { asc, desc, eq, inArray } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import { JournalSequence, type ThreadListItem } from "@artisan/protocol";

import { DecodeThreadProjection } from "../threads/internal/thread-projection";
import { Database } from "./database";
import { JournalEvents, OrchestrationCoordinators, Threads } from "./tables";
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
		/** One coordinator per thread, so the launch policy joins one-to-one. */
		const Lookup = (thread_id: string) => {
			const candidate_ids = thread_id.startsWith("thread_")
				? [thread_id]
				: [thread_id, `thread_${thread_id}`];
			return database.client
				.select()
				.from(Threads)
				.leftJoin(
					OrchestrationCoordinators,
					eq(OrchestrationCoordinators.thread_id, Threads.thread_id),
				)
				.where(inArray(Threads.thread_id, candidate_ids))
				.limit(candidate_ids.length)
				.pipe(
					Effect.flatMap((rows) => {
						const row =
							rows.find((candidate) => candidate.threads.thread_id === thread_id) ??
							rows.at(0);
						return row
							? DecodeThreadProjection(
									row.threads,
									row.orchestration_coordinators ?? undefined,
								).pipe(Effect.map(Option.some))
							: Effect.succeed(Option.none());
					}),
					Effect.mapError(normalize_thread_read_model_error),
				);
		};

		const Snapshot = () =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const thread_rows = yield* transaction
							.select()
							.from(Threads)
							.leftJoin(
								OrchestrationCoordinators,
								eq(OrchestrationCoordinators.thread_id, Threads.thread_id),
							)
							.orderBy(asc(Threads.created_at), asc(Threads.thread_id));
						const [watermark] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const journal_sequence = watermark
							? yield* DecodePersistedJournalSequence(watermark.journal_sequence)
							: 0;
						const threads = yield* Effect.forEach(thread_rows, (row) =>
							DecodeThreadProjection(
								row.threads,
								row.orchestration_coordinators ?? undefined,
							),
						);

						return { journal_sequence, threads };
					}),
				)
				.pipe(Effect.mapError(normalize_thread_read_model_error));

		return { Lookup, Snapshot };
	}),
);
