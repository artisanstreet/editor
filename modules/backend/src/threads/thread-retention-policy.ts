import { eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";

import {
	ThreadRetentionPolicy,
	type CommandEnvelope,
	type EventEnvelope,
	type ThreadRetentionPolicyUpdatedEvent,
} from "@artisan/protocol";

import { settings_scope_id, settings_stream_id } from "../settings/internal-scope";

export const thread_retention_policy_thread_id = settings_scope_id("thread-retention");
const thread_retention_policy_stream_id = settings_stream_id("thread-retention");

import { Database } from "../persistence/database";
import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	ThreadRetentionPolicies,
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

/** Returns one durable retention-policy update and its canonical event. */
export interface ThreadRetentionPolicyAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Owns the opinionated global retention setting and its durable updates. */
export class ThreadRetentionPolicyService extends Context.Service<
	ThreadRetentionPolicyService,
	{
		readonly Read: Effect.Effect<ThreadRetentionPolicy, JournalStoreError>;
		readonly Update: (
			command: CommandEnvelope,
		) => Effect.Effect<ThreadRetentionPolicyAcceptance, JournalStoreError>;
	}
>()("Artisan/ThreadRetentionPolicyService") {}

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

const DecodePolicy = (input: unknown) =>
	Schema.decodeUnknownEffect(ThreadRetentionPolicy, {
		onExcessProperty: "error",
	})(input).pipe(
		Effect.mapError(
			() =>
				new JournalInvariantError({
					message: "The stored thread retention policy is invalid",
				}),
		),
	);

export const ThreadRetentionPolicyServiceLive = Layer.effect(
	ThreadRetentionPolicyService,
	Effect.gen(function* () {
		const database = yield* Database;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const Read = database.client
			.select({
				enabled: ThreadRetentionPolicies.enabled,
				inactivity_days: ThreadRetentionPolicies.inactivity_days,
			})
			.from(ThreadRetentionPolicies)
			.where(eq(ThreadRetentionPolicies.policy_id, 1))
			.limit(1)
			.pipe(
				Effect.flatMap(([policy]) =>
					policy
						? DecodePolicy(policy)
						: Effect.fail(
								new JournalInvariantError({
									message: "The thread retention policy row is missing",
								}),
							),
				),
				Effect.mapError(normalize_error),
			);

		const Update = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				if (
					command.payload.type !== "thread.retention.update" ||
					command.thread_id !== thread_retention_policy_thread_id
				) {
					return yield* new JournalInvariantError({
						message: "Retention updates require the canonical policy scope",
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
						const stream_id = thread_retention_policy_stream_id;
						const [stream] = yield* transaction
							.select({ last_sequence: EventStreams.last_sequence })
							.from(EventStreams)
							.where(eq(EventStreams.stream_id, stream_id))
							.limit(1);
						const sequence = (stream?.last_sequence ?? 0) + 1;
						const event_payload = {
							policy: {
								enabled: payload.enabled,
								inactivity_days: payload.inactivity_days,
							},
							type: "thread.retention.updated" as const,
						} satisfies ThreadRetentionPolicyUpdatedEvent;

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
						yield* transaction
							.update(ThreadRetentionPolicies)
							.set({
								enabled: payload.enabled,
								inactivity_days: payload.inactivity_days,
								updated_at: accepted_at,
							})
							.where(eq(ThreadRetentionPolicies.policy_id, 1));

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
								message: `Retention event ${event_id} returned no inserted row`,
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
							message: `Retention command ${command.message_id} has no event`,
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
