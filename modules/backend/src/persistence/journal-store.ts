import { asc, desc, eq, gt, lte } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	JournalSequence,
	StreamCursor,
	type CommandEnvelope,
	type RawOrigin,
	type ThreadCreatedEvent,
} from "@artisan/protocol";

import { Database } from "./database";
import { EventStreams, JournalCommands, JournalEvents, Threads } from "./schema";
import { JournalNotifier } from "./journal-notifier";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

export interface ThreadCreateAcceptance {
	readonly agent_id?: string;
	readonly event_id: string;
	readonly journal_sequence: number;
	readonly occurred_at: string;
	readonly payload: ThreadCreatedEvent;
	readonly raw_origin?: RawOrigin;
	readonly run_id?: string;
	readonly sequence: number;
	readonly status: "accepted" | "duplicate";
	readonly stream_id: string;
	readonly thread_id: string;
}

export interface ReplayRequest {
	readonly after_journal_sequence: number;
	readonly stream_cursors?: ReadonlyArray<StreamCursor>;
}

export class CommandIdConflict extends Data.TaggedError("CommandIdConflict")<{
	readonly message_id: string;
}> {}

export class ThreadAlreadyExists extends Data.TaggedError("ThreadAlreadyExists")<{
	readonly thread_id: string;
}> {}

export class JournalInvariantError extends Data.TaggedError("JournalInvariantError")<{
	readonly message: string;
}> {}

export class JournalStoreFailure extends Data.TaggedError("JournalStoreFailure")<{
	readonly cause: unknown;
}> {}

export type JournalStoreError =
	| CommandIdConflict
	| ThreadAlreadyExists
	| JournalInvariantError
	| JournalStoreFailure;

export class JournalStore extends Context.Service<
	JournalStore,
	{
		readonly AcceptThreadCreate: (
			command: CommandEnvelope,
		) => Effect.Effect<ThreadCreateAcceptance, JournalStoreError>;
		readonly ReadCurrentCursors: () => Effect.Effect<
			ReadonlyArray<StreamCursor>,
			JournalStoreError
		>;
		readonly ReadReplay: (
			request: ReplayRequest,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, JournalStoreError>;
		readonly ReadWatermark: () => Effect.Effect<number, JournalStoreError>;
		readonly ValidateReplayPoint: (
			request: Required<ReplayRequest>,
		) => Effect.Effect<void, JournalStoreError>;
	}
>()("Artisan/JournalStore") {}

interface PersistedStreamSequence {
	readonly sequence: unknown;
	readonly stream_id: unknown;
}

function normalize_journal_error(error: unknown): JournalStoreError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof ThreadAlreadyExists ||
		error instanceof JournalInvariantError
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

const DecodeJournalSequence = (value: unknown, context: string) =>
	Schema.decodeUnknownEffect(JournalSequence)(value).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `${context} is not a valid journal sequence`,
				}),
		),
	);

const DecodePersistedJournalSequence = (value: unknown, context: string) =>
	DecodeJournalSequence(value, context).pipe(
		Effect.flatMap((sequence) => {
			if (sequence === 0) {
				return new JournalInvariantError({
					message: `${context} must be greater than zero`,
				});
			}

			return Effect.succeed(sequence);
		}),
	);

const DecodeStreamCursor = (value: unknown, context: string) =>
	Schema.decodeUnknownEffect(StreamCursor, { onExcessProperty: "error" })(value).pipe(
		Effect.flatMap((cursor) => {
			if (cursor.sequence === 0) {
				return new JournalInvariantError({
					message: `${context} must be greater than zero`,
				});
			}

			return Effect.succeed(cursor);
		}),
		Effect.mapError((error) =>
			error instanceof JournalInvariantError
				? error
				: new JournalInvariantError({
						message: `${context} is not a valid stream cursor`,
					}),
		),
	);

const DecodeReplayCursor = (value: unknown) =>
	Schema.decodeUnknownEffect(StreamCursor, { onExcessProperty: "error" })(value).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "Replay stream cursor is not valid",
				}),
		),
	);

const ParsePersistedJson = (json: string, context: string) =>
	Effect.try({
		try: () => JSON.parse(json) as unknown,
		catch: () =>
			new JournalInvariantError({
				message: `${context} contains invalid JSON`,
			}),
	});

const ReconstructEventEnvelope = (event: {
	readonly agent_id: string | null;
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly event_id: string;
	readonly event_type: string;
	readonly journal_sequence: number;
	readonly occurred_at: string;
	readonly origin: string;
	readonly payload_json: string;
	readonly raw_origin_json: string | null;
	readonly run_id: string | null;
	readonly schema_version: number;
	readonly sequence: number;
	readonly stream_id: string;
	readonly thread_id: string;
}) =>
	Effect.gen(function* () {
		const payload = yield* ParsePersistedJson(event.payload_json, "Event payload");
		const raw_origin =
			event.raw_origin_json === null
				? undefined
				: yield* ParsePersistedJson(event.raw_origin_json, "Event raw origin");

		const envelope = yield* Schema.decodeUnknownEffect(EventEnvelope, {
			onExcessProperty: "error",
		})({
			protocol_version: 1,
			schema_version: event.schema_version,
			kind: "event",
			message_id: event.event_id,
			correlation_id: event.correlation_id,
			causation_id: event.causation_id,
			stream_id: event.stream_id,
			sequence: event.sequence,
			journal_sequence: event.journal_sequence,
			thread_id: event.thread_id,
			...(event.run_id === null ? {} : { run_id: event.run_id }),
			...(event.agent_id === null ? {} : { agent_id: event.agent_id }),
			origin: event.origin,
			...(raw_origin === undefined ? {} : { raw_origin }),
			sent_at: event.occurred_at,
			payload,
		}).pipe(
			Effect.mapError(
				() =>
					new JournalInvariantError({
						message: `Event ${event.event_id} does not match the protocol schema`,
					}),
			),
		);

		if (envelope.journal_sequence === 0 || envelope.sequence === 0) {
			return yield* new JournalInvariantError({
				message: `Event ${event.event_id} has a zero persisted sequence`,
			});
		}

		if (event.event_type !== envelope.payload.type) {
			return yield* new JournalInvariantError({
				message: `Event ${event.event_id} payload type does not match its stored type`,
			});
		}

		return envelope;
	});

const DeriveStreamCursors = (rows: ReadonlyArray<PersistedStreamSequence>) =>
	Effect.gen(function* () {
		const cursors = yield* Effect.forEach(rows, (row) =>
			DecodeStreamCursor(
				{
					sequence: row.sequence,
					stream_id: row.stream_id,
				},
				"Persisted stream sequence",
			),
		);
		const cursors_by_stream = new Map<string, StreamCursor>();

		for (const cursor of cursors) {
			const previous_cursor = cursors_by_stream.get(cursor.stream_id);
			const expected_sequence = (previous_cursor?.sequence ?? 0) + 1;

			if (cursor.sequence !== expected_sequence) {
				return yield* new JournalInvariantError({
					message: `Stream ${cursor.stream_id} is not contiguous`,
				});
			}

			cursors_by_stream.set(cursor.stream_id, cursor);
		}

		return [...cursors_by_stream.values()];
	});

const ContinueStreamCursors = (
	initial_cursors: ReadonlyArray<StreamCursor>,
	events: ReadonlyArray<EventEnvelope>,
) =>
	Effect.gen(function* () {
		const cursors_by_stream = new Map(
			initial_cursors.map((cursor) => [cursor.stream_id, cursor]),
		);

		for (const event of events) {
			const previous_cursor = cursors_by_stream.get(event.stream_id);
			const expected_sequence = (previous_cursor?.sequence ?? 0) + 1;

			if (event.sequence !== expected_sequence) {
				return yield* new JournalInvariantError({
					message: `Event ${event.message_id} breaks stream ${event.stream_id} continuity`,
				});
			}

			cursors_by_stream.set(event.stream_id, {
				sequence: event.sequence,
				stream_id: event.stream_id,
			});
		}

		return [...cursors_by_stream.values()];
	});

const AssertMatchingCursors = (
	actual_cursors: ReadonlyArray<StreamCursor>,
	expected_cursors: ReadonlyArray<StreamCursor>,
	context: string,
) =>
	Effect.gen(function* () {
		const actual_by_stream = new Map<string, StreamCursor>();

		for (const cursor of actual_cursors) {
			if (actual_by_stream.has(cursor.stream_id)) {
				return yield* new JournalInvariantError({
					message: `${context} contains a duplicate stream cursor`,
				});
			}

			actual_by_stream.set(cursor.stream_id, cursor);
		}

		if (actual_by_stream.size !== expected_cursors.length) {
			return yield* new JournalInvariantError({
				message: `${context} does not match the journal streams`,
			});
		}

		for (const expected_cursor of expected_cursors) {
			const actual_cursor = actual_by_stream.get(expected_cursor.stream_id);

			if (actual_cursor?.sequence !== expected_cursor.sequence) {
				return yield* new JournalInvariantError({
					message: `${context} does not match stream ${expected_cursor.stream_id}`,
				});
			}
		}
	});

export const JournalStoreLive = Layer.effect(
	JournalStore,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const ReadWatermark = () =>
			database.client
				.select({ journal_sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([watermark]) => {
						if (!watermark) {
							return Effect.succeed(0);
						}

						return DecodePersistedJournalSequence(
							watermark.journal_sequence,
							"Journal watermark",
						);
					}),
					Effect.mapError(normalize_journal_error),
				);

		const ReadCurrentCursors = () =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const current_cursor_rows = yield* transaction
							.select({
								sequence: EventStreams.last_sequence,
								stream_id: EventStreams.stream_id,
							})
							.from(EventStreams)
							.orderBy(asc(EventStreams.stream_id));
						const event_rows = yield* transaction
							.select({
								sequence: JournalEvents.stream_sequence,
								stream_id: JournalEvents.stream_id,
							})
							.from(JournalEvents)
							.orderBy(
								asc(JournalEvents.stream_id),
								asc(JournalEvents.stream_sequence),
							);
						const current_cursors = yield* Effect.forEach(current_cursor_rows, (row) =>
							DecodeStreamCursor(row, "Current stream cursor"),
						);
						const expected_cursors = yield* DeriveStreamCursors(event_rows);

						yield* AssertMatchingCursors(
							current_cursors,
							expected_cursors,
							"Current stream cursors",
						);

						return current_cursors;
					}),
				)
				.pipe(Effect.mapError(normalize_journal_error));

		const ReadReplay = (request: ReplayRequest) =>
			database.client
				.transaction((transaction) =>
					Effect.gen(function* () {
						const after_journal_sequence = yield* DecodeJournalSequence(
							request.after_journal_sequence,
							"Replay journal sequence",
						);
						const stream_cursors = request.stream_cursors
							? yield* Effect.forEach(request.stream_cursors, DecodeReplayCursor)
							: undefined;
						const [watermark] = yield* transaction
							.select({ journal_sequence: JournalEvents.sequence })
							.from(JournalEvents)
							.orderBy(desc(JournalEvents.sequence))
							.limit(1);
						const current_watermark = watermark
							? yield* DecodePersistedJournalSequence(
									watermark.journal_sequence,
									"Journal watermark",
								)
							: 0;

						if (after_journal_sequence > current_watermark) {
							return yield* new JournalInvariantError({
								message:
									"Replay journal sequence is ahead of the journal watermark",
							});
						}

						const boundary_event_rows = yield* transaction
							.select({
								sequence: JournalEvents.stream_sequence,
								stream_id: JournalEvents.stream_id,
							})
							.from(JournalEvents)
							.where(lte(JournalEvents.sequence, after_journal_sequence))
							.orderBy(
								asc(JournalEvents.stream_id),
								asc(JournalEvents.stream_sequence),
							);
						const boundary_cursors = yield* DeriveStreamCursors(boundary_event_rows);

						if (stream_cursors) {
							yield* AssertMatchingCursors(
								stream_cursors,
								boundary_cursors,
								"Replay stream cursors",
							);
						}

						const replay_rows = yield* transaction
							.select({
								agent_id: JournalEvents.agent_id,
								causation_id: JournalEvents.causation_id,
								correlation_id: JournalEvents.correlation_id,
								event_id: JournalEvents.event_id,
								event_type: JournalEvents.event_type,
								journal_sequence: JournalEvents.sequence,
								occurred_at: JournalEvents.occurred_at,
								origin: JournalEvents.origin,
								payload_json: JournalEvents.payload_json,
								raw_origin_json: JournalEvents.raw_origin_json,
								run_id: JournalEvents.run_id,
								schema_version: JournalEvents.schema_version,
								sequence: JournalEvents.stream_sequence,
								stream_id: JournalEvents.stream_id,
								thread_id: JournalEvents.thread_id,
							})
							.from(JournalEvents)
							.where(gt(JournalEvents.sequence, after_journal_sequence))
							.orderBy(asc(JournalEvents.sequence));
						const events = yield* Effect.forEach(replay_rows, ReconstructEventEnvelope);
						const replay_cursors = yield* ContinueStreamCursors(
							boundary_cursors,
							events,
						);
						const current_cursor_rows = yield* transaction
							.select({
								sequence: EventStreams.last_sequence,
								stream_id: EventStreams.stream_id,
							})
							.from(EventStreams)
							.orderBy(asc(EventStreams.stream_id));
						const current_cursors = yield* Effect.forEach(current_cursor_rows, (row) =>
							DecodeStreamCursor(row, "Current stream cursor"),
						);

						yield* AssertMatchingCursors(
							current_cursors,
							replay_cursors,
							"Current stream cursors",
						);

						return events;
					}),
				)
				.pipe(Effect.mapError(normalize_journal_error));

		const AcceptThreadCreate = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const payload_json = JSON.stringify(command.payload);
				const raw_origin_json = command.raw_origin
					? JSON.stringify(command.raw_origin)
					: null;

				return yield* database.client
					.transaction((transaction) =>
						Effect.gen(function* () {
							const [existing_command] = yield* transaction
								.select({
									agent_id: JournalCommands.agent_id,
									causation_id: JournalCommands.causation_id,
									origin: JournalCommands.origin,
									payload_json: JournalCommands.payload_json,
									raw_origin_json: JournalCommands.raw_origin_json,
									run_id: JournalCommands.run_id,
									schema_version: JournalCommands.schema_version,
									sent_at: JournalCommands.sent_at,
									thread_id: JournalCommands.thread_id,
								})
								.from(JournalCommands)
								.where(eq(JournalCommands.message_id, command.message_id))
								.limit(1);

							if (existing_command) {
								const command_matches =
									existing_command.payload_json === payload_json &&
									existing_command.thread_id === command.thread_id &&
									existing_command.run_id === (command.run_id ?? null) &&
									existing_command.agent_id === (command.agent_id ?? null) &&
									existing_command.causation_id ===
										(command.causation_id ?? null) &&
									existing_command.origin === command.origin &&
									existing_command.raw_origin_json === raw_origin_json &&
									existing_command.schema_version === command.schema_version &&
									existing_command.sent_at === command.sent_at;

								if (!command_matches) {
									return yield* new CommandIdConflict({
										message_id: command.message_id,
									});
								}

								const [existing_event] = yield* transaction
									.select({
										event_id: JournalEvents.event_id,
										journal_sequence: JournalEvents.sequence,
										occurred_at: JournalEvents.occurred_at,
										sequence: JournalEvents.stream_sequence,
										stream_id: JournalEvents.stream_id,
									})
									.from(JournalEvents)
									.where(eq(JournalEvents.correlation_id, command.message_id))
									.limit(1);

								if (!existing_event) {
									return yield* new JournalInvariantError({
										message: `Command ${command.message_id} has no event`,
									});
								}

								return {
									...(command.agent_id ? { agent_id: command.agent_id } : {}),
									event_id: existing_event.event_id,
									journal_sequence: existing_event.journal_sequence,
									occurred_at: existing_event.occurred_at,
									payload: {
										type: "thread.created" as const,
										title: command.payload.title,
									},
									...(command.raw_origin
										? { raw_origin: command.raw_origin }
										: {}),
									...(command.run_id ? { run_id: command.run_id } : {}),
									sequence: existing_event.sequence,
									status: "duplicate" as const,
									stream_id: existing_event.stream_id,
									thread_id: command.thread_id,
								};
							}

							const [existing_thread] = yield* transaction
								.select({ thread_id: Threads.thread_id })
								.from(Threads)
								.where(eq(Threads.thread_id, command.thread_id))
								.limit(1);

							if (existing_thread) {
								return yield* new ThreadAlreadyExists({
									thread_id: command.thread_id,
								});
							}

							const event_id = yield* metadata.MakeId("event");
							const occurred_at = yield* metadata.Now;
							const stream_id = `thread:${command.thread_id}`;
							const event_payload = {
								type: "thread.created" as const,
								title: command.payload.title,
							};

							yield* transaction.insert(JournalCommands).values({
								accepted_at: occurred_at,
								agent_id: command.agent_id ?? null,
								causation_id: command.causation_id ?? null,
								message_id: command.message_id,
								origin: command.origin,
								payload_json,
								payload_type: command.payload.type,
								raw_origin_json,
								run_id: command.run_id ?? null,
								schema_version: command.schema_version,
								sent_at: command.sent_at,
								status: "accepted",
								thread_id: command.thread_id,
							});

							yield* transaction.insert(Threads).values({
								created_at: occurred_at,
								thread_id: command.thread_id,
								title: command.payload.title,
								updated_at: occurred_at,
							});

							yield* transaction.insert(EventStreams).values({
								last_sequence: 1,
								stream_id,
							});

							const [inserted_event] = yield* transaction
								.insert(JournalEvents)
								.values({
									agent_id: command.agent_id ?? null,
									causation_id: command.message_id,
									correlation_id: command.message_id,
									event_id,
									event_type: event_payload.type,
									occurred_at,
									origin: "backend",
									payload_json: JSON.stringify(event_payload),
									raw_origin_json,
									run_id: command.run_id ?? null,
									schema_version: 1,
									stream_id,
									stream_sequence: 1,
									thread_id: command.thread_id,
								})
								.returning({ sequence: JournalEvents.sequence });

							return {
								...(command.agent_id ? { agent_id: command.agent_id } : {}),
								event_id,
								journal_sequence: inserted_event!.sequence,
								occurred_at,
								payload: event_payload,
								...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
								...(command.run_id ? { run_id: command.run_id } : {}),
								sequence: 1,
								status: "accepted" as const,
								stream_id,
								thread_id: command.thread_id,
							};
						}),
					)
					.pipe(
						Effect.mapError(normalize_journal_error),
						Effect.tap((acceptance) =>
							acceptance.status === "accepted"
								? notifier.Publish(acceptance.journal_sequence)
								: Effect.void,
						),
					);
			});
		const ValidateReplayPoint = (request: Required<ReplayRequest>) =>
			ReadReplay(request).pipe(Effect.asVoid);

		return {
			AcceptThreadCreate,
			ReadCurrentCursors,
			ReadReplay,
			ReadWatermark,
			ValidateReplayPoint,
		};
	}),
);
