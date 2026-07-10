import { eq } from "drizzle-orm";
import { Context, Data, Effect, Layer } from "effect";

import type { CommandEnvelope, RawOrigin, ThreadCreatedEvent } from "@artisan/protocol";

import { Database } from "./database";
import { EventStreams, JournalCommands, JournalEvents, Threads } from "./schema";
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
	}
>()("Artisan/JournalStore") {}

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

export const JournalStoreLive = Layer.effect(
	JournalStore,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

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
					.pipe(Effect.mapError(normalize_journal_error));
			});

		return { AcceptThreadCreate };
	}),
);
