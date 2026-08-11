import { and, asc, eq, notExists } from "drizzle-orm";
import { Effect, Schema } from "effect";

import { CommandPayload } from "@artisan/protocol";

import type { DatabaseClient } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import {
	OrchestrationFailure,
	type OrchestrationError,
	type OutboxKind,
	type PendingWork,
} from "./contracts";
import { HydrateImageAttachments, ImageAttachmentsFor } from "./message-attachments";
import { AuthoritativeThreadSendMessageCommand } from "./message-command";
import { DecodePersistedJson, PersistedJsonValue, PersistedRawOrigin } from "./storage-codec";
import type { PendingWork as PendingWorkContract } from "./contracts";
import {
	JournalCommands,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	ThreadErasureClaims,
} from "../tables";

type RuntimeMetadata = Parameters<typeof AppendJournalEventInTransaction>[1];

interface JournalNotifier {
	readonly Publish: (journal_sequence: number) => Effect.Effect<void>;
}

const NormalizeError = (error: unknown): OrchestrationError =>
	error instanceof OrchestrationFailure ? error : new OrchestrationFailure({ cause: error });

type PersistedPayload = PendingWorkContract["payload"];

const DecodePersistedPayload = (
	value: unknown,
): Effect.Effect<PersistedPayload, OrchestrationFailure> =>
	Effect.gen(function* () {
		if (
			typeof value === "object" &&
			value !== null &&
			"type" in value &&
			value.type === "thread.send_message"
		) {
			return yield* Schema.decodeUnknownEffect(AuthoritativeThreadSendMessageCommand)(
				value,
			).pipe(Effect.mapError((cause) => new OrchestrationFailure({ cause })));
		}

		const decoded = yield* Schema.decodeUnknownEffect(CommandPayload)(value).pipe(
			Effect.mapError((cause) => new OrchestrationFailure({ cause })),
		);
		if (decoded.type === "thread.send_message") {
			return yield* new OrchestrationFailure({
				cause: new Error("Persisted message payload is missing Forge authority"),
			});
		}
		return decoded as PersistedPayload;
	});

/** Decodes the exact Forge-owned message command that a steer outbox row represents. */
export const ReadAuthoritativeSteeringPayload = (
	database: DatabaseClient,
	command_id: string,
	payload_json: string,
) =>
	Effect.gen(function* () {
		const persisted_payload = yield* DecodePersistedJson(PersistedJsonValue, payload_json);
		const decoded_payload = yield* DecodePersistedPayload(persisted_payload);
		if (decoded_payload.type !== "thread.send_message") {
			return yield* new OrchestrationFailure({
				cause: new Error("Steering outbox payload must be a thread message"),
			});
		}
		const payload = yield* HydrateImageAttachments(database, decoded_payload);
		const [command] = yield* database
			.select({ raw_origin_json: JournalCommands.raw_origin_json })
			.from(JournalCommands)
			.where(eq(JournalCommands.message_id, command_id))
			.limit(1);
		const raw_origin = command?.raw_origin_json
			? yield* DecodePersistedJson(PersistedRawOrigin, command.raw_origin_json)
			: undefined;

		return { payload, raw_origin };
	});

/** Owns the durable dispatch queue while leaving command acceptance in the repository. */
export const make_outbox_operations = (
	database: DatabaseClient,
	metadata: RuntimeMetadata,
	notifier: JournalNotifier,
) => {
	const GetPending = () =>
		database
			.select({
				agent_id: OrchestrationOutbox.agent_id,
				command_id: OrchestrationOutbox.command_id,
				engine_id: OrchestrationRuns.engine_id,
				kind: OrchestrationOutbox.kind,
				payload_json: OrchestrationOutbox.payload_json,
				run_id: OrchestrationOutbox.run_id,
				status: OrchestrationRuns.status,
				thread_id: OrchestrationOutbox.thread_id,
				working_directory: OrchestrationRuns.working_directory,
			})
			.from(OrchestrationOutbox)
			.innerJoin(OrchestrationRuns, eq(OrchestrationOutbox.run_id, OrchestrationRuns.run_id))
			.where(
				and(
					eq(OrchestrationOutbox.status, "pending"),
					notExists(
						database
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(
								eq(ThreadErasureClaims.thread_id, OrchestrationOutbox.thread_id),
							),
					),
				),
			)
			.orderBy(asc(OrchestrationOutbox.created_at), asc(OrchestrationOutbox.command_id))
			.pipe(
				Effect.flatMap((rows) =>
					Effect.forEach(
						rows.filter((row) => row.kind !== "start" || row.status === "queued"),
						(row) =>
							Effect.gen(function* () {
								const value = yield* DecodePersistedJson(
									PersistedJsonValue,
									row.payload_json,
								);
								const decoded = yield* DecodePersistedPayload(value);
								const payload: PersistedPayload =
									decoded.type === "thread.send_message"
										? yield* HydrateImageAttachments(database, decoded)
										: decoded;

								return {
									agent_id: row.agent_id,
									command_id: row.command_id,
									engine_id: row.engine_id,
									kind: row.kind as OutboxKind,
									payload,
									run_id: row.run_id,
									thread_id: row.thread_id,
									working_directory: row.working_directory,
								} satisfies PendingWork;
							}),
					),
				),
				Effect.mapError(NormalizeError),
			);

	const CompleteOutbox = (command_id: string) =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;
			const events = yield* database.transaction((transaction) =>
				Effect.gen(function* () {
					const [outbox] = yield* transaction
						.update(OrchestrationOutbox)
						.set({ status: "delivered", updated_at })
						.where(
							and(
								eq(OrchestrationOutbox.command_id, command_id),
								eq(OrchestrationOutbox.status, "dispatching"),
							),
						)
						.returning();
					if (!outbox || outbox.kind !== "steer") return [];

					const [message] = yield* transaction
						.select()
						.from(OrchestrationMessages)
						.where(eq(OrchestrationMessages.command_id, command_id))
						.limit(1);
					if (
						!message ||
						message.delivery !== "steering" ||
						message.run_id === null ||
						message.agent_id === null
					) {
						return [];
					}
					const { payload, raw_origin } = yield* ReadAuthoritativeSteeringPayload(
						transaction,
						command_id,
						outbox.payload_json,
					);

					return [
						yield* AppendJournalEventInTransaction(transaction, metadata, {
							agent_id: message.agent_id,
							causation_id: command_id,
							correlation_id: command_id,
							payload: {
								message_id: message.message_id,
								...(payload.attachments === undefined
									? {}
									: { attachments: ImageAttachmentsFor(payload) }),
								...(payload.content === undefined
									? {}
									: { content: payload.content }),
								...(payload.mentioned_projects === undefined
									? {}
									: { mentioned_projects: payload.mentioned_projects }),
								text: payload.text,
								type: "thread.message_steering",
								working_directory: payload.working_directory,
							},
							...(raw_origin === undefined ? {} : { raw_origin }),
							run_id: message.run_id,
							thread_id: message.thread_id,
						}),
						yield* AppendJournalEventInTransaction(transaction, metadata, {
							agent_id: message.agent_id,
							causation_id: command_id,
							correlation_id: command_id,
							payload: {
								message_id: message.message_id,
								outcome: "steered",
								run_id: message.run_id,
								type: "thread.message_routed",
							},
							run_id: message.run_id,
							thread_id: message.thread_id,
						}),
					];
				}),
			);
			const latest_event = events.at(-1);
			if (latest_event !== undefined) yield* notifier.Publish(latest_event.journal_sequence);
		}).pipe(Effect.mapError(NormalizeError));

	const ClaimOutbox = (command_id: string) =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;
			const claimed = yield* database
				.update(OrchestrationOutbox)
				.set({ status: "dispatching", updated_at })
				.where(
					and(
						eq(OrchestrationOutbox.command_id, command_id),
						eq(OrchestrationOutbox.status, "pending"),
						notExists(
							database
								.select({ thread_id: ThreadErasureClaims.thread_id })
								.from(ThreadErasureClaims)
								.where(
									eq(
										ThreadErasureClaims.thread_id,
										OrchestrationOutbox.thread_id,
									),
								),
						),
					),
				)
				.returning({ command_id: OrchestrationOutbox.command_id });

			return claimed.length === 1;
		}).pipe(Effect.mapError(NormalizeError));

	const MarkOutboxUndeliverable = (command_id: string) =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;

			yield* database
				.update(OrchestrationOutbox)
				.set({ status: "undeliverable", updated_at })
				.where(eq(OrchestrationOutbox.command_id, command_id));
		}).pipe(Effect.mapError(NormalizeError));

	return {
		ClaimOutbox,
		CompleteOutbox,
		GetPending,
		MarkOutboxUndeliverable,
	};
};
