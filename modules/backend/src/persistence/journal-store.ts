import { asc, desc, eq, gt, lte } from "drizzle-orm";
import { Context, Data, Effect, Layer, Option, Schema } from "effect";

import {
	EventEnvelope,
	EventPayload,
	JournalSequence,
	StreamCursor,
	type CommandEnvelope,
	type RawOrigin,
	type ThreadCreatedEvent,
} from "@artisan/protocol";

import { Database, type DatabaseClient } from "./database";
import { EventStreams, JournalCommands, JournalEvents, Projects, Threads } from "./tables";
import { JournalNotifier } from "./journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";
import { ApplyJournalEvent } from "../conversation/index.ts";
import { RecordThreadActivity } from "../threads/internal/thread-activity";
import { is_settings_scope_id } from "../settings/internal-scope";

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

export interface JournalBaseline {
	readonly event_cursors: ReadonlyArray<StreamCursor>;
	readonly journal_sequence: number;
}

export interface JournalEventInput {
	readonly agent_id?: string;
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly idempotency_key?: string;
	readonly payload: EventPayload;
	readonly raw_origin?: RawOrigin;
	readonly run_id?: string;
	readonly thread_id: string;
}

/**
 * Appends one event while preserving the caller's database transaction.
 *
 * Repositories with additional atomic writes use this capability instead of
 * reproducing journal stream sequencing, activity, and projection invariants.
 */
export const AppendJournalEventInTransaction = (
	transaction: DatabaseClient,
	metadata: {
		readonly MakeId: (prefix: "event") => Effect.Effect<string>;
		readonly Now: Effect.Effect<string>;
	},
	input: JournalEventInput,
) =>
	Effect.gen(function* () {
		const stream_id = `thread:${input.thread_id}`;
		const [stream] = yield* transaction
			.select({ last_sequence: EventStreams.last_sequence })
			.from(EventStreams)
			.where(eq(EventStreams.stream_id, stream_id))
			.limit(1);
		const sequence = (stream?.last_sequence ?? 0) + 1;
		const event_id = yield* metadata.MakeId("event");
		const occurred_at = yield* metadata.Now;

		yield* RecordThreadActivity(transaction, input.thread_id, occurred_at, input.payload);

		if (stream) {
			yield* transaction
				.update(EventStreams)
				.set({ last_sequence: sequence })
				.where(eq(EventStreams.stream_id, stream_id));
		} else {
			yield* transaction.insert(EventStreams).values({
				last_sequence: sequence,
				stream_id,
			});
		}

		const [inserted] = yield* transaction
			.insert(JournalEvents)
			.values({
				agent_id: input.agent_id ?? null,
				causation_id: input.causation_id,
				correlation_id: input.correlation_id,
				event_id,
				event_type: input.payload.type,
				...(input.idempotency_key === undefined
					? {}
					: { idempotency_key: input.idempotency_key }),
				occurred_at,
				origin: "backend",
				payload_json: JSON.stringify(input.payload),
				raw_origin_json: input.raw_origin ? JSON.stringify(input.raw_origin) : null,
				run_id: input.run_id ?? null,
				schema_version: 1,
				stream_id,
				stream_sequence: sequence,
				thread_id: input.thread_id,
			})
			.returning({ journal_sequence: JournalEvents.sequence });
		if (inserted === undefined)
			return yield* new JournalInvariantError({
				message: `Journal append for event ${event_id} returned no inserted row`,
			});

		const event = {
			...(input.agent_id ? { agent_id: input.agent_id } : {}),
			causation_id: input.causation_id,
			correlation_id: input.correlation_id,
			journal_sequence: inserted.journal_sequence,
			kind: "event" as const,
			message_id: event_id,
			origin: "backend" as const,
			payload: input.payload,
			protocol_version: 1 as const,
			...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
			...(input.run_id ? { run_id: input.run_id } : {}),
			schema_version: 1 as const,
			sequence,
			sent_at: occurred_at,
			stream_id,
			thread_id: input.thread_id,
		} satisfies EventEnvelope;
		yield* ApplyJournalEvent(transaction, event) as Effect.Effect<unknown, unknown, never>;
		return event;
	});

export class CommandIdConflict extends Data.TaggedError("CommandIdConflict")<{
	readonly message_id: string;
}> {}

export class ThreadAlreadyExists extends Data.TaggedError("ThreadAlreadyExists")<{
	readonly thread_id: string;
}> {}

export class ThreadReservedScope extends Data.TaggedError("ThreadReservedScope")<{
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
	| ThreadReservedScope
	| JournalInvariantError
	| JournalStoreFailure;

export class JournalStore extends Context.Service<
	JournalStore,
	{
		readonly AcceptThreadCreate: (
			command: CommandEnvelope,
		) => Effect.Effect<ThreadCreateAcceptance, JournalStoreError>;
		readonly AppendEvent: (
			input: JournalEventInput,
		) => Effect.Effect<EventEnvelope, JournalStoreError>;
		readonly FindCommandThreadId: (
			message_id: string,
		) => Effect.Effect<Option.Option<string>, JournalStoreError>;
		readonly ReadCorrelatedEvents: (
			correlation_id: string,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, JournalStoreError>;
		readonly ReadBaseline: () => Effect.Effect<JournalBaseline, JournalStoreError>;
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
		error instanceof ThreadReservedScope ||
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
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `${context} contains invalid JSON`,
				}),
		),
	);

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

const ReadBaselineInTransaction = (transaction: DatabaseClient) =>
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
			.orderBy(asc(JournalEvents.stream_id), asc(JournalEvents.stream_sequence));
		const [watermark] = yield* transaction
			.select({ journal_sequence: JournalEvents.sequence })
			.from(JournalEvents)
			.orderBy(desc(JournalEvents.sequence))
			.limit(1);
		const event_cursors = yield* Effect.forEach(current_cursor_rows, (row) =>
			DecodeStreamCursor(row, "Current stream cursor"),
		);
		const expected_cursors = yield* DeriveStreamCursors(event_rows);

		yield* AssertMatchingCursors(event_cursors, expected_cursors, "Current stream cursors");

		return {
			event_cursors,
			journal_sequence: watermark
				? yield* DecodePersistedJournalSequence(
						watermark.journal_sequence,
						"Journal watermark",
					)
				: 0,
		} satisfies JournalBaseline;
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

		const ReadCorrelatedEvents = (correlation_id: string) =>
			database.client
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
				.where(eq(JournalEvents.correlation_id, correlation_id))
				.orderBy(asc(JournalEvents.sequence))
				.pipe(
					Effect.flatMap((events) => Effect.forEach(events, ReconstructEventEnvelope)),
					Effect.mapError(normalize_journal_error),
				);

		const AppendEvent = (input: JournalEventInput) =>
			database.client
				.transaction((transaction) =>
					AppendJournalEventInTransaction(transaction, metadata, input),
				)
				.pipe(
					Effect.mapError(normalize_journal_error),
					Effect.tap((event) => notifier.Publish(event.journal_sequence)),
				);

		const ReadBaseline = () =>
			database.client
				.transaction(ReadBaselineInTransaction)
				.pipe(Effect.mapError(normalize_journal_error));
		const ReadCurrentCursors = () =>
			ReadBaseline().pipe(Effect.map((baseline) => baseline.event_cursors));

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
				if (command.payload.type !== "thread.create") {
					return yield* new JournalInvariantError({
						message: "Thread creation requires a thread.create command payload",
					});
				}

				if (is_settings_scope_id(command.thread_id)) {
					return yield* new ThreadReservedScope({ thread_id: command.thread_id });
				}

				const payload = command.payload;
				const payload_json = JSON.stringify(payload);
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
									existing_command.schema_version === command.schema_version;

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
										title: payload.title,
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
							const project =
								payload.project_id === undefined
									? undefined
									: (yield* transaction
											.select()
											.from(Projects)
											.where(eq(Projects.project_id, payload.project_id))
											.limit(1))[0];
							if (payload.project_id !== undefined && project === undefined) {
								return yield* new JournalInvariantError({
									message: `Project ${payload.project_id} is not attached to Forge`,
								});
							}
							const project_ref =
								project === undefined
									? undefined
									: {
											display_name: project.display_name,
											project_id: project.project_id,
											root_path: project.root_path,
										};
							const event_payload = {
								type: "thread.created" as const,
								title: payload.title,
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
								activity_version: 0,
								affinity_version: project_ref === undefined ? 0 : 1,
								created_at: occurred_at,
								current_goal: payload.title,
								last_activity_at: occurred_at,
								reader_activity_at: occurred_at,
								live_status: "Idle",
								metadata_version: 0,
								pinned: false,
								...(project_ref === undefined
									? {}
									: {
											linked_projects_json: JSON.stringify([project_ref]),
											primary_project_id: project_ref.project_id,
											primary_project_json: JSON.stringify(project_ref),
											project_locked: true,
										}),
								thread_id: command.thread_id,
								title: payload.title,
								title_locked: false,
								title_source: "initial",
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
							if (inserted_event === undefined)
								return yield* new JournalInvariantError({
									message: `Thread creation event ${event_id} returned no inserted row`,
								});

							return {
								...(command.agent_id ? { agent_id: command.agent_id } : {}),
								event_id,
								journal_sequence: inserted_event.sequence,
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
		const FindCommandThreadId = (message_id: string) =>
			database.client
				.select({ thread_id: JournalCommands.thread_id })
				.from(JournalCommands)
				.where(eq(JournalCommands.message_id, message_id))
				.limit(1)
				.pipe(
					Effect.map(([command]) =>
						command === undefined
							? Option.none<string>()
							: Option.some(command.thread_id),
					),
					Effect.mapError(normalize_journal_error),
				);
		const ValidateReplayPoint = (request: Required<ReplayRequest>) =>
			ReadReplay(request).pipe(Effect.asVoid);

		return {
			AcceptThreadCreate,
			AppendEvent,
			FindCommandThreadId,
			ReadBaseline,
			ReadCurrentCursors,
			ReadCorrelatedEvents,
			ReadReplay,
			ReadWatermark,
			ValidateReplayPoint,
		};
	}),
);
