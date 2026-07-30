import { and, asc, eq, or } from "drizzle-orm";
import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	EventEnvelope,
	Identifier,
	ThreadMetadataRefineCommand,
	ThreadListItem,
	type CommandEnvelope,
	type EventPayload,
	type ProjectAffinityScore,
	type ProjectRef,
	type RawOrigin,
	type ThreadProjectRehomeSuggestion,
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
} from "../persistence/tables";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";
import { DecodeThreadProjection } from "./internal/thread-projection";

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

interface MetadataOperation {
	readonly agent_id?: string;
	readonly causation_id?: string;
	readonly event_causation_id: string;
	readonly message_id: string;
	readonly origin: "backend" | "frontend";
	readonly payload: ThreadMetadataCommand;
	readonly raw_origin?: RawOrigin;
	readonly retry_policy: "exact" | "source";
	readonly run_id?: string;
	readonly schema_version: number;
	readonly sent_at?: string;
	readonly thread_id: string;
}

interface StoredMetadataCommand {
	readonly agent_id: string | null;
	readonly causation_id: string | null;
	readonly origin: string;
	readonly payload_json: string;
	readonly payload_type: string;
	readonly raw_origin_json: string | null;
	readonly run_id: string | null;
	readonly schema_version: number;
	readonly sent_at: string;
	readonly thread_id: string;
}

interface StoredThreadProjection {
	readonly activity_version: number;
	readonly affinity_version: number;
	readonly archived_at: string | null;
	readonly created_at: string;
	readonly current_goal: string | null;
	readonly last_activity_at: string;
	readonly live_status: string;
	readonly linked_projects: ReadonlyArray<ProjectRef>;
	readonly metadata_version: number;
	readonly pinned: boolean;
	readonly primary_project: ProjectRef | null;
	readonly project_affinity_scores: ReadonlyArray<ProjectAffinityScore>;
	readonly project_locked: boolean;
	readonly rename_suggestion: string | null;
	readonly rehome_suggestion: ThreadProjectRehomeSuggestion | null;
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

/** Carries one backend-owned refinement proposal with durable source-event identity. */
export const ThreadMetadataRefinementIntent = Schema.Struct({
	operation_id: Identifier,
	payload: ThreadMetadataRefineCommand,
	source_event_id: Identifier,
	thread_id: Identifier,
});

export type ThreadMetadataRefinementIntent = typeof ThreadMetadataRefinementIntent.Type;

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
		readonly Refine: (
			intent: ThreadMetadataRefinementIntent,
		) => Effect.Effect<ThreadMetadataAcceptance, ThreadMetadataError>;
		readonly WasRefined: (
			source_event_id: string,
			thread_id: string,
		) => Effect.Effect<boolean, ThreadMetadataError>;
	}
>()("Artisan/ThreadMetadataRepository") {}

const DecodeThreadListItem = (input: StoredThreadProjection) => {
	const {
		archived_at,
		current_goal,
		primary_project,
		rename_suggestion,
		rehome_suggestion,
		...required
	} = input;

	return Schema.decodeUnknownEffect(ThreadListItem, {
		onExcessProperty: "error",
	})({
		...required,
		...(archived_at === null ? {} : { archived_at }),
		...(current_goal === null ? {} : { current_goal }),
		...(primary_project === null ? {} : { primary_project }),
		...(rename_suggestion === null ? {} : { rename_suggestion }),
		...(rehome_suggestion === null ? {} : { rehome_suggestion }),
	}).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: `Thread ${input.thread_id} metadata is invalid`,
				}),
		),
	);
};

function operation_matches(operation: MetadataOperation, existing: StoredMetadataCommand) {
	const identity_matches =
		existing.agent_id === (operation.agent_id ?? null) &&
		existing.causation_id === (operation.causation_id ?? null) &&
		existing.origin === operation.origin &&
		existing.payload_type === operation.payload.type &&
		existing.raw_origin_json ===
			(operation.raw_origin ? JSON.stringify(operation.raw_origin) : null) &&
		existing.run_id === (operation.run_id ?? null) &&
		existing.schema_version === operation.schema_version &&
		existing.thread_id === operation.thread_id;

	if (!identity_matches || operation.retry_policy === "source") {
		return identity_matches;
	}

	return (
		existing.payload_json === JSON.stringify(operation.payload) &&
		existing.sent_at === operation.sent_at
	);
}

function is_metadata_command(
	payload: CommandEnvelope["payload"],
): payload is ThreadMetadataCommand {
	return (
		payload.type === "thread.activity.record" ||
		payload.type === "thread.archive" ||
		payload.type === "thread.metadata.refine" ||
		payload.type === "thread.pin" ||
		payload.type === "thread.rename" ||
		payload.type === "thread.restore" ||
		payload.type === "thread.unpin"
	);
}

const DecodeJson = <A>(
	value: string,
	schema: Schema.ConstraintDecoder<A, never>,
	context: string,
) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(value).pipe(
		Effect.mapError(
			() => new JournalInvariantError({ message: `${context} JSON is malformed` }),
		),
		Effect.flatMap(
			Schema.decodeUnknownEffect(schema, {
				onExcessProperty: "error",
			}),
		),
		Effect.mapError(
			() => new JournalInvariantError({ message: `${context} does not match its schema` }),
		),
	);

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
					...(payload.mentioned_projects === undefined
						? {}
						: { mentioned_projects: payload.mentioned_projects }),
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

		const ReadDuplicate = (message_id: string) =>
			database.client
				.select()
				.from(JournalEvents)
				.where(
					and(
						eq(JournalEvents.correlation_id, message_id),
						or(
							eq(JournalEvents.event_type, "thread.metadata.updated"),
							eq(JournalEvents.event_type, "thread.refinement.ignored"),
						),
					),
				)
				.orderBy(asc(JournalEvents.sequence))
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) => {
						if (!row) {
							return Effect.fail(
								new JournalInvariantError({
									message: `Command ${message_id} has no metadata event`,
								}),
							);
						}

						return Effect.all({
							payload: DecodeJson(
								row.payload_json,
								EventEnvelope.fields.payload,
								`Command ${message_id} metadata event payload`,
							),
							raw_origin:
								row.raw_origin_json === null
									? Effect.succeed(undefined)
									: DecodeJson(
											row.raw_origin_json,
											EventEnvelope.fields.raw_origin,
											`Command ${message_id} metadata event raw origin`,
										),
						}).pipe(
							Effect.flatMap(({ payload, raw_origin }) =>
								Schema.decodeUnknownEffect(EventEnvelope, {
									onExcessProperty: "error",
								})({
									...(row.agent_id === null ? {} : { agent_id: row.agent_id }),
									causation_id: row.causation_id,
									correlation_id: row.correlation_id,
									journal_sequence: row.sequence,
									kind: "event",
									message_id: row.event_id,
									origin: row.origin,
									payload,
									protocol_version: 1,
									...(raw_origin === undefined ? {} : { raw_origin }),
									...(row.run_id === null ? {} : { run_id: row.run_id }),
									schema_version: row.schema_version,
									sequence: row.stream_sequence,
									sent_at: row.occurred_at,
									stream_id: row.stream_id,
									thread_id: row.thread_id,
								}),
							),
							Effect.mapError((error) =>
								error instanceof JournalInvariantError
									? error
									: new JournalInvariantError({
											message: `Command ${message_id} metadata event is invalid`,
										}),
							),
						);
					}),
					Effect.mapError(normalize_error),
				);

		const AcceptOperation = (operation: MetadataOperation) =>
			Effect.gen(function* () {
				const payload = operation.payload;
				const accepted = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing_command] = yield* transaction
							.select()
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, operation.message_id))
							.limit(1);

						if (existing_command) {
							if (!operation_matches(operation, existing_command)) {
								return yield* new CommandIdConflict({
									message_id: operation.message_id,
								});
							}

							return {
								_tag: "Duplicate" as const,
								message_id: operation.message_id,
							};
						}

						if (operation.retry_policy === "source") {
							const source_event_id = operation.causation_id;

							if (!source_event_id) {
								return yield* new JournalInvariantError({
									message: "Automatic thread refinement has no source event",
								});
							}

							const [existing_source] = yield* transaction
								.select()
								.from(JournalCommands)
								.where(
									and(
										eq(JournalCommands.causation_id, source_event_id),
										eq(JournalCommands.origin, "backend"),
										eq(JournalCommands.payload_type, "thread.metadata.refine"),
										eq(JournalCommands.thread_id, operation.thread_id),
									),
								)
								.limit(1);

							if (existing_source) {
								if (!operation_matches(operation, existing_source)) {
									return yield* new CommandIdConflict({
										message_id: operation.message_id,
									});
								}

								return {
									_tag: "Duplicate" as const,
									message_id: existing_source.message_id,
								};
							}
						}

						const [blocked] = yield* transaction
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, operation.thread_id))
							.limit(1);
						const [deleted] = yield* transaction
							.select({ thread_id: ThreadTombstones.thread_id })
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, operation.thread_id))
							.limit(1);
						const [current] = yield* transaction
							.select()
							.from(Threads)
							.where(eq(Threads.thread_id, operation.thread_id))
							.limit(1);

						if (!current || blocked || deleted) {
							return yield* new ThreadNotFound({ thread_id: operation.thread_id });
						}

						const current_projection = yield* DecodeThreadProjection(current);
						const stored_current: StoredThreadProjection = {
							...current_projection,
							affinity_version: current_projection.affinity_version,
							archived_at: current_projection.archived_at ?? null,
							current_goal: current_projection.current_goal ?? null,
							linked_projects: current_projection.linked_projects,
							primary_project: current_projection.primary_project ?? null,
							project_affinity_scores: current_projection.project_affinity_scores,
							project_locked: current_projection.project_locked,
							rename_suggestion: current_projection.rename_suggestion ?? null,
							rehome_suggestion: current_projection.rehome_suggestion ?? null,
						};
						const occurred_at = yield* metadata.Now;
						const transition = yield* MakeTransition(
							stored_current,
							payload,
							occurred_at,
						);
						const event_payload = transition.event_payload;
						const stream_id = `thread:${operation.thread_id}`;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);

						if (!stream) {
							return yield* new JournalInvariantError({
								message: `Thread ${operation.thread_id} has no event stream`,
							});
						}

						const sequence = stream.last_sequence + 1;
						const event_id = yield* metadata.MakeId("event");

						yield* transaction.insert(JournalCommands).values({
							accepted_at: occurred_at,
							agent_id: operation.agent_id ?? null,
							causation_id: operation.causation_id ?? null,
							message_id: operation.message_id,
							origin: operation.origin,
							payload_json: JSON.stringify(operation.payload),
							payload_type: operation.payload.type,
							raw_origin_json: operation.raw_origin
								? JSON.stringify(operation.raw_origin)
								: null,
							run_id: operation.run_id ?? null,
							schema_version: operation.schema_version,
							sent_at: operation.sent_at ?? occurred_at,
							status: "accepted",
							thread_id: operation.thread_id,
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
								.where(eq(Threads.thread_id, operation.thread_id));
						}
						yield* transaction
							.update(EventStreams)
							.set({ last_sequence: sequence })
							.where(eq(EventStreams.stream_id, stream_id));
						const [event_row] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: operation.agent_id ?? null,
								causation_id: operation.event_causation_id,
								correlation_id: operation.message_id,
								event_id,
								event_type: event_payload.type,
								occurred_at,
								origin: "backend",
								payload_json: JSON.stringify(event_payload),
								raw_origin_json: operation.raw_origin
									? JSON.stringify(operation.raw_origin)
									: null,
								run_id: operation.run_id ?? null,
								schema_version: 1,
								stream_id,
								stream_sequence: sequence,
								thread_id: operation.thread_id,
							})
							.returning({ journal_sequence: JournalEvents.sequence });
						if (event_row === undefined)
							return yield* new JournalInvariantError({
								message: `Metadata event ${event_id} returned no inserted row`,
							});

						return {
							_tag: "Accepted" as const,
							event: {
								...(operation.agent_id ? { agent_id: operation.agent_id } : {}),
								causation_id: operation.event_causation_id,
								correlation_id: operation.message_id,
								journal_sequence: event_row.journal_sequence,
								kind: "event" as const,
								message_id: event_id,
								origin: "backend" as const,
								payload: event_payload,
								protocol_version: 1 as const,
								...(operation.raw_origin
									? { raw_origin: operation.raw_origin }
									: {}),
								...(operation.run_id ? { run_id: operation.run_id } : {}),
								schema_version: 1 as const,
								sequence,
								sent_at: occurred_at,
								stream_id,
								thread_id: operation.thread_id,
							} satisfies EventEnvelope,
						};
					}),
				);

				if (accepted._tag === "Duplicate") {
					return {
						event: yield* ReadDuplicate(accepted.message_id),
						status: "duplicate" as const,
					};
				}

				yield* notifier.Publish(accepted.event.journal_sequence);

				return { event: accepted.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		const Accept = (command: CommandEnvelope) => {
			if (!is_metadata_command(command.payload)) {
				return Effect.fail(
					new JournalInvariantError({
						message: "Thread metadata received an unsupported command",
					}),
				);
			}

			return AcceptOperation({
				...(command.agent_id ? { agent_id: command.agent_id } : {}),
				...(command.causation_id ? { causation_id: command.causation_id } : {}),
				event_causation_id: command.message_id,
				message_id: command.message_id,
				origin: "frontend",
				payload: command.payload,
				...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
				retry_policy: "exact",
				...(command.run_id ? { run_id: command.run_id } : {}),
				schema_version: command.schema_version,
				sent_at: command.sent_at,
				thread_id: command.thread_id,
			});
		};

		const Refine = (intent: ThreadMetadataRefinementIntent) =>
			Schema.decodeUnknownEffect(ThreadMetadataRefinementIntent, {
				onExcessProperty: "error",
			})(intent).pipe(
				Effect.mapError(
					() =>
						new JournalInvariantError({
							message: "Automatic thread metadata refinement intent is invalid",
						}),
				),
				Effect.flatMap((decoded) =>
					AcceptOperation({
						causation_id: decoded.source_event_id,
						event_causation_id: decoded.source_event_id,
						message_id: decoded.operation_id,
						origin: "backend",
						payload: decoded.payload,
						retry_policy: "source",
						schema_version: 1,
						thread_id: decoded.thread_id,
					}),
				),
			);

		const WasRefined = (source_event_id: string, thread_id: string) =>
			database.client
				.select({ message_id: JournalCommands.message_id })
				.from(JournalCommands)
				.where(
					and(
						eq(JournalCommands.causation_id, source_event_id),
						eq(JournalCommands.origin, "backend"),
						eq(JournalCommands.payload_type, "thread.metadata.refine"),
						eq(JournalCommands.thread_id, thread_id),
					),
				)
				.limit(1)
				.pipe(
					Effect.map((rows) => rows.length > 0),
					Effect.mapError(normalize_error),
				);

		return { Accept, Refine, WasRefined };
	}),
);
