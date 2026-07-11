import { asc, eq } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	ThreadListItem,
	type CommandEnvelope,
	type EventPayload,
	type RawOrigin,
	type ThreadMetadataUpdatedEvent,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ThreadErasureClaims,
	Threads,
	ThreadTombstones,
} from "../persistence/schema";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

type ThreadMetadataCommand = Extract<
	CommandEnvelope["payload"],
	{
		readonly type:
			| "thread.activity.record"
			| "thread.archive"
			| "thread.metadata.refine"
			| "thread.pin"
			| "thread.rename"
			| "thread.restore"
			| "thread.unpin";
	}
>;

interface StoredThreadProjection {
	readonly activity_version: number;
	readonly archived_at: string | null;
	readonly created_at: string;
	readonly current_goal: string | null;
	readonly last_activity_at: string;
	readonly live_status: string;
	readonly metadata_version: number;
	readonly pinned: boolean;
	readonly rename_suggestion: string | null;
	readonly thread_id: string;
	readonly title: string;
	readonly title_locked: boolean;
	readonly title_source: string;
	readonly updated_at: string;
}

/** Returns one durable metadata command outcome and its canonical event. */
export interface ThreadMetadataAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Reports a metadata command targeting an absent, erasing, or erased thread. */
export class ThreadNotFound extends Data.TaggedError("ThreadNotFound")<{
	readonly thread_id: string;
}> {}

export type ThreadMetadataError = JournalStoreError | ThreadNotFound;

/** Persists versioned thread identity and lifecycle transitions atomically. */
export class ThreadMetadataRepository extends Context.Service<
	ThreadMetadataRepository,
	{
		readonly Accept: (
			command: CommandEnvelope,
		) => Effect.Effect<ThreadMetadataAcceptance, ThreadMetadataError>;
	}
>()("Artisan/ThreadMetadataRepository") {}

const DecodeThreadListItem = (input: StoredThreadProjection) => {
	const { archived_at, current_goal, rename_suggestion, ...required } = input;

	return Schema.decodeUnknownEffect(ThreadListItem, {
		onExcessProperty: "error",
	})({
		...required,
		...(archived_at === null ? {} : { archived_at }),
		...(current_goal === null ? {} : { current_goal }),
		...(rename_suggestion === null ? {} : { rename_suggestion }),
	}).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Thread ${input.thread_id} metadata is invalid`,
				}),
		),
	);
};

function command_matches(
	command: CommandEnvelope,
	existing: {
		readonly agent_id: string | null;
		readonly causation_id: string | null;
		readonly origin: string;
		readonly payload_json: string;
		readonly raw_origin_json: string | null;
		readonly run_id: string | null;
		readonly schema_version: number;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) {
	return (
		existing.payload_json === JSON.stringify(command.payload) &&
		existing.thread_id === command.thread_id &&
		existing.run_id === (command.run_id ?? null) &&
		existing.agent_id === (command.agent_id ?? null) &&
		existing.causation_id === (command.causation_id ?? null) &&
		existing.origin === command.origin &&
		existing.raw_origin_json ===
			(command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
		existing.schema_version === command.schema_version &&
		existing.sent_at === command.sent_at
	);
}

function max_timestamp(left: string, right: string) {
	return left < right ? right : left;
}

const MakeTransition = (
	current: StoredThreadProjection,
	payload: ThreadMetadataCommand,
	occurred_at: string,
) =>
	Effect.gen(function* () {
		if (payload.type === "thread.metadata.refine") {
			if (
				payload.basis_activity_version !== current.activity_version ||
				payload.basis_metadata_version !== current.metadata_version
			) {
				return {
					event_payload: {
						basis_activity_version: payload.basis_activity_version,
						basis_metadata_version: payload.basis_metadata_version,
						type: "thread.refinement.ignored" as const,
					} satisfies EventPayload,
				};
			}

			const projection = yield* DecodeThreadListItem({
				...current,
				current_goal: payload.current_goal ?? null,
				live_status: payload.live_status,
				metadata_version: current.metadata_version + 1,
				rename_suggestion: payload.rename_suggestion ?? null,
				title:
					current.title_locked || payload.title === undefined
						? current.title
						: payload.title,
				title_source:
					current.title_locked || payload.title === undefined
						? current.title_source
						: "automatic",
				updated_at: max_timestamp(current.updated_at, occurred_at),
			});

			return {
				event_payload: {
					change: "metadata",
					thread: projection,
					type: "thread.metadata.updated",
				} satisfies ThreadMetadataUpdatedEvent,
				projection,
			};
		}

		const activity_kind =
			payload.type === "thread.activity.record"
				? payload.activity_kind
				: payload.type === "thread.rename"
					? "renamed"
					: payload.type === "thread.pin"
						? "pinned"
						: payload.type === "thread.unpin"
							? "unpinned"
							: payload.type === "thread.archive"
								? "archived"
								: "restored";
		const change =
			payload.type === "thread.activity.record"
				? "activity"
				: payload.type === "thread.rename"
					? "rename"
					: payload.type === "thread.pin"
						? "pin"
						: payload.type === "thread.unpin"
							? "unpin"
							: payload.type === "thread.archive"
								? "archive"
								: "restore";
		const changes_metadata = payload.type !== "thread.activity.record";
		const projection = yield* DecodeThreadListItem({
			...current,
			activity_version: current.activity_version + 1,
			archived_at:
				payload.type === "thread.archive"
					? occurred_at
					: payload.type === "thread.restore"
						? null
						: current.archived_at,
			last_activity_at: max_timestamp(current.last_activity_at, occurred_at),
			metadata_version: current.metadata_version + (changes_metadata ? 1 : 0),
			pinned:
				payload.type === "thread.pin"
					? true
					: payload.type === "thread.unpin"
						? false
						: current.pinned,
			rename_suggestion: payload.type === "thread.rename" ? null : current.rename_suggestion,
			title: payload.type === "thread.rename" ? payload.title : current.title,
			title_locked: payload.type === "thread.rename" ? true : current.title_locked,
			title_source: payload.type === "thread.rename" ? "manual" : current.title_source,
			updated_at: max_timestamp(current.updated_at, occurred_at),
		});

		return {
			event_payload: {
				activity_kind,
				change,
				thread: projection,
				type: "thread.metadata.updated",
			} satisfies ThreadMetadataUpdatedEvent,
			projection,
		};
	});

function normalize_error(error: unknown): ThreadMetadataError {
	if (
		error instanceof CommandIdConflict ||
		error instanceof JournalInvariantError ||
		error instanceof ThreadNotFound
	) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

export const ThreadMetadataRepositoryLive = Layer.effect(
	ThreadMetadataRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const ReadDuplicate = (command: CommandEnvelope) =>
			database.client
				.select()
				.from(JournalEvents)
				.where(eq(JournalEvents.correlation_id, command.message_id))
				.orderBy(asc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) => {
						if (!row) {
							return Effect.fail(
								new JournalInvariantError({
									message: `Command ${command.message_id} has no metadata event`,
								}),
							);
						}

						return Schema.decodeUnknownEffect(EventEnvelope, {
							onExcessProperty: "error",
						})({
							agent_id: row.agent_id ?? undefined,
							causation_id: row.causation_id,
							correlation_id: row.correlation_id,
							journal_sequence: row.sequence,
							kind: "event",
							message_id: row.event_id,
							origin: row.origin,
							payload: JSON.parse(row.payload_json) as unknown,
							protocol_version: 1,
							raw_origin:
								row.raw_origin_json === null
									? undefined
									: (JSON.parse(row.raw_origin_json) as RawOrigin),
							run_id: row.run_id ?? undefined,
							schema_version: row.schema_version,
							sequence: row.stream_sequence,
							sent_at: row.occurred_at,
							stream_id: row.stream_id,
							thread_id: row.thread_id,
						}).pipe(
							Effect.mapError(
								() =>
									new JournalInvariantError({
										message: `Command ${command.message_id} metadata event is invalid`,
									}),
							),
						);
					}),
					Effect.mapError(normalize_error),
				);

		const Accept = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const payload = command.payload as ThreadMetadataCommand;
				const accepted = yield* database.client.transaction((transaction) =>
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
							if (!command_matches(command, existing_command)) {
								return yield* new CommandIdConflict({
									message_id: command.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const [blocked] = yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, command.thread_id))
							.limit(1);
						const [deleted] = yield* transaction
							.select({ thread_id: ThreadTombstones.thread_id })
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, command.thread_id))
							.limit(1);
						const [current] = yield* transaction
							.select()
							.from(Threads)
							.where(eq(Threads.thread_id, command.thread_id))
							.limit(1);

						if (!current || blocked || deleted) {
							return yield* new ThreadNotFound({ thread_id: command.thread_id });
						}

						const occurred_at = yield* metadata.Now;
						const transition = yield* MakeTransition(current, payload, occurred_at);
						const event_payload = transition.event_payload;
						const stream_id = `thread:${command.thread_id}`;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);

						if (!stream) {
							return yield* new JournalInvariantError({
								message: `Thread ${command.thread_id} has no event stream`,
							});
						}

						const sequence = stream.last_sequence + 1;
						const event_id = yield* metadata.MakeId("event");

						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							agent_id: command.agent_id ?? null,
							causation_id: command.causation_id ?? null,
							message_id: command.message_id,
							origin: command.origin,
							payload_json: JSON.stringify(command.payload),
							payload_type: command.payload.type,
							raw_origin_json: command.raw_origin
								? JSON.stringify(command.raw_origin)
								: null,
							run_id: command.run_id ?? null,
							schema_version: command.schema_version,
							sent_at: command.sent_at,
							status: "accepted",
							thread_id: command.thread_id,
						});
						if (transition.projection) {
							const projection = transition.projection;

							yield* transaction
								.update(Threads)
								.set({
									activity_version: projection.activity_version,
									archived_at: projection.archived_at ?? null,
									current_goal: projection.current_goal ?? null,
									last_activity_at: projection.last_activity_at,
									live_status: projection.live_status,
									metadata_version: projection.metadata_version,
									pinned: projection.pinned,
									rename_suggestion: projection.rename_suggestion ?? null,
									title: projection.title,
									title_locked: projection.title_locked,
									title_source: projection.title_source,
									updated_at: projection.updated_at,
								})
								.where(eq(Threads.thread_id, command.thread_id));
						}
						yield* transaction
							.update(EventStreams)
							.set({ last_sequence: sequence })
							.where(eq(EventStreams.stream_id, stream_id));
						const [event_row] = yield* transaction
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
								raw_origin_json: command.raw_origin
									? JSON.stringify(command.raw_origin)
									: null,
								run_id: command.run_id ?? null,
								schema_version: 1,
								stream_id,
								stream_sequence: sequence,
								thread_id: command.thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });

						return {
							_tag: "Accepted" as const,
							event: {
								...(command.agent_id ? { agent_id: command.agent_id } : {}),
								causation_id: command.message_id,
								correlation_id: command.message_id,
								journal_sequence: event_row!.journal_sequence,
								kind: "event" as const,
								message_id: event_id,
								origin: "backend" as const,
								payload: event_payload,
								protocol_version: 1 as const,
								...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
								...(command.run_id ? { run_id: command.run_id } : {}),
								schema_version: 1 as const,
								sequence,
								sent_at: occurred_at,
								stream_id,
								thread_id: command.thread_id,
							} satisfies EventEnvelope,
						};
					}),
				);

				if (accepted._tag === "Duplicate") {
					return { event: yield* ReadDuplicate(command), status: "duplicate" as const };
				}

				yield* notifier.Publish(accepted.event.journal_sequence);

				return { event: accepted.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return { Accept };
	}),
);
