import { asc, eq } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import {
	type CommandEnvelope,
	type EventEnvelope,
	type ModelFavoritesSnapshot,
	type ModelFavoritesUpdatedEvent,
} from "@artisan/protocol";

import { settings_scope_id, settings_stream_id } from "../settings/internal-scope";

export const model_favorites_thread_id = settings_scope_id("model-favorites");
const model_favorites_stream_id = settings_stream_id("model-favorites");

import { Database } from "../persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ModelFavorites,
} from "../persistence/tables";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStore,
	JournalStoreFailure,
	type JournalStoreError,
} from "../persistence/journal-store";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";

/** Returns one durable favorite change and its canonical event. */
export interface ModelFavoritesAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Owns the starred model set and its durable updates. */
export class ModelFavoritesService extends Context.Service<
	ModelFavoritesService,
	{
		readonly Read: Effect.Effect<ModelFavoritesSnapshot, JournalStoreError>;
		readonly Update: (
			command: CommandEnvelope,
		) => Effect.Effect<ModelFavoritesAcceptance, JournalStoreError>;
	}
>()("Artisan/ModelFavoritesService") {}

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

function normalize_error(error: unknown): JournalStoreError {
	if (error instanceof CommandIdConflict || error instanceof JournalInvariantError) {
		return error;
	}

	return new JournalStoreFailure({ cause: error });
}

export const ModelFavoritesServiceLive = Layer.effect(
	ModelFavoritesService,
	Effect.gen(function* () {
		const database = yield* Database;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		/**
		 * Oldest star first. The picker floats favorites to the top in this
		 * order, so a stable insertion order keeps the list from reshuffling
		 * itself between openings.
		 */
		const ReadFavorites = database.client
			.select({ model_id: ModelFavorites.model_id })
			.from(ModelFavorites)
			.orderBy(asc(ModelFavorites.favorited_at), asc(ModelFavorites.model_id));

		const Read = ReadFavorites.pipe(
			Effect.map((rows) => ({ model_ids: rows.map((row) => row.model_id) })),
			Effect.mapError(normalize_error),
		);

		const Update = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				if (
					command.payload.type !== "model.favorite.update" ||
					command.thread_id !== model_favorites_thread_id
				) {
					return yield* new JournalInvariantError({
						message: "Favorite updates require the canonical favorites scope",
					});
				}

				const payload = command.payload;
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
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

						if (existing) {
							if (!command_matches(command, existing)) {
								return yield* new CommandIdConflict({
									message_id: command.message_id,
								});
							}

							return { _tag: "Duplicate" as const };
						}

						const accepted_at = yield* metadata.Now;
						const event_id = yield* metadata.MakeId("event");
						const stream_id = model_favorites_stream_id;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;

						/**
						 * The command names the end state, so applying it twice is
						 * the same as applying it once and a duplicate delivery can
						 * never flip a star back.
						 */
						if (payload.favorite) {
							yield* transaction
								.insert(ModelFavorites)
								.values({ favorited_at: accepted_at, model_id: payload.model_id })
								.onConflictDoNothing();
						} else {
							yield* transaction
								.delete(ModelFavorites)
								.where(eq(ModelFavorites.model_id, payload.model_id));
						}

						const rows = yield* transaction
							.select({ model_id: ModelFavorites.model_id })
							.from(ModelFavorites)
							.orderBy(
								asc(ModelFavorites.favorited_at),
								asc(ModelFavorites.model_id),
							);
						const event_payload = {
							favorites: { model_ids: rows.map((row) => row.model_id) },
							type: "model.favorites.updated" as const,
						} satisfies ModelFavoritesUpdatedEvent;

						yield* transaction.insert(JournalCommands).values({
							accepted_at,
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

						const [event_row] = yield* transaction
							.insert(JournalEvents)
							.values({
								agent_id: command.agent_id ?? null,
								causation_id: command.message_id,
								correlation_id: command.message_id,
								event_id,
								event_type: event_payload.type,
								occurred_at: accepted_at,
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
						if (event_row === undefined)
							return yield* new JournalInvariantError({
								message: `Model favorites event ${event_id} returned no inserted row`,
							});
						const event: EventEnvelope = {
							...(command.agent_id ? { agent_id: command.agent_id } : {}),
							causation_id: command.message_id,
							correlation_id: command.message_id,
							journal_sequence: event_row.journal_sequence,
							kind: "event",
							message_id: event_id,
							origin: "backend",
							payload: event_payload,
							protocol_version: 1,
							...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
							...(command.run_id ? { run_id: command.run_id } : {}),
							schema_version: 1,
							sequence,
							sent_at: accepted_at,
							stream_id,
							thread_id: command.thread_id,
						};

						return { _tag: "Accepted" as const, event };
					}),
				);

				if (result._tag === "Duplicate") {
					const [event] = yield* journal.ReadCorrelatedEvents(command.message_id);

					if (!event) {
						return yield* new JournalInvariantError({
							message: `Favorite command ${command.message_id} has no event`,
						});
					}

					return { event, status: "duplicate" as const };
				}

				yield* notifier.Publish(result.event.journal_sequence);

				return { event: result.event, status: "accepted" as const };
			}).pipe(Effect.mapError(normalize_error));

		return { Read, Update };
	}),
);
