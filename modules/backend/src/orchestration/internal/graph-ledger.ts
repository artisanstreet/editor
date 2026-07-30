import { asc, eq } from "drizzle-orm";
import { Effect } from "effect";

import type { CommandEnvelope, EventEnvelope, EventPayload, RawOrigin } from "@artisan/protocol";

import {
	EventStreams,
	JournalCommands,
	JournalEvents,
	OrchestrationGraphCommands,
	OrchestrationGroups,
} from "../../persistence/tables";
import {
	AgentGraphCommandConflict,
	AgentGraphInvalid,
	type AcceptedAgentGraphCommand,
} from "../agent-graph-model";
import { command_matches, type GraphContext, type GraphTransaction } from "./graph-context";
import type { PersistedGraphCodecs } from "./persisted-graph-codecs";
import { RecordThreadActivity } from "../../threads/internal/thread-activity";

export interface GraphLedger {
	readonly append_event: (
		transaction: GraphTransaction,
		input: {
			readonly agent_id: string;
			readonly causation_id: string;
			readonly correlation_id: string;
			readonly group_id: string;
			readonly payload: EventPayload;
			readonly raw_origin?: RawOrigin;
			readonly run_id?: string;
			readonly thread_id: string;
		},
	) => Effect.Effect<EventEnvelope, unknown>;
	readonly command_acceptance: (
		group_id: string,
		events: ReadonlyArray<EventEnvelope>,
		status: "accepted" | "duplicate",
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphInvalid>;
	readonly insert_journal_command: (
		transaction: GraphTransaction,
		command: CommandEnvelope,
		accepted_at: string,
	) => Effect.Effect<void, unknown>;
	readonly publish_events: (events: ReadonlyArray<EventEnvelope>) => Effect.Effect<void>;
	readonly read_correlated_events: (
		transaction: GraphTransaction,
		correlation_id: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
	readonly read_existing_command: (
		transaction: GraphTransaction,
		command: CommandEnvelope,
	) => Effect.Effect<typeof OrchestrationGraphCommands.$inferSelect | undefined, unknown>;
}

/** Owns canonical event append, command identity, and journal cursor advancement. */
export function make_graph_ledger(
	context: GraphContext,
	codecs: PersistedGraphCodecs,
): GraphLedger {
	const { metadata, notifier } = context;

	const read_correlated_events = (transaction: GraphTransaction, correlation_id: string) =>
		transaction
			.select()
			.from(JournalEvents)
			.where(eq(JournalEvents.correlation_id, correlation_id))
			.orderBy(asc(JournalEvents.sequence))
			.pipe(Effect.flatMap((rows) => Effect.forEach(rows, codecs.decode_event_row)));

	const append_event = (
		transaction: GraphTransaction,
		input: Parameters<GraphLedger["append_event"]>[1],
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
				yield* transaction
					.insert(EventStreams)
					.values({ last_sequence: sequence, stream_id });
			}

			const [inserted] = yield* transaction
				.insert(JournalEvents)
				.values({
					agent_id: input.agent_id,
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					event_id,
					event_type: input.payload.type,
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
				return yield* new AgentGraphInvalid({
					message: `Graph event ${event_id} returned no inserted row`,
				});
			const journal_sequence = inserted.journal_sequence;
			const [group] = yield* transaction
				.select({ version: OrchestrationGroups.version })
				.from(OrchestrationGroups)
				.where(eq(OrchestrationGroups.group_id, input.group_id))
				.limit(1);

			if (!group) {
				return yield* new AgentGraphInvalid({
					message: `Graph event lost orchestration group ${input.group_id}`,
				});
			}

			yield* transaction
				.update(OrchestrationGroups)
				.set({
					journal_sequence,
					updated_at: occurred_at,
					version: group.version + 1,
				})
				.where(eq(OrchestrationGroups.group_id, input.group_id));

			return {
				agent_id: input.agent_id,
				causation_id: input.causation_id,
				correlation_id: input.correlation_id,
				journal_sequence,
				kind: "event",
				message_id: event_id,
				origin: "backend",
				payload: input.payload,
				protocol_version: 1,
				...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
				...(input.run_id ? { run_id: input.run_id } : {}),
				schema_version: 1,
				sequence,
				sent_at: occurred_at,
				stream_id,
				thread_id: input.thread_id,
			} satisfies EventEnvelope;
		});

	const publish_events = (events: ReadonlyArray<EventEnvelope>) => {
		const latest_event = events.at(-1);
		return latest_event === undefined
			? Effect.void
			: notifier.Publish(latest_event.journal_sequence);
	};

	const insert_journal_command = (
		transaction: GraphTransaction,
		command: CommandEnvelope,
		accepted_at: string,
	) =>
		transaction
			.insert(JournalCommands)
			.values({
				accepted_at,
				agent_id: command.agent_id ?? null,
				causation_id: command.causation_id ?? null,
				message_id: command.message_id,
				origin: command.origin,
				payload_json: JSON.stringify(command.payload),
				payload_type: command.payload.type,
				raw_origin_json: command.raw_origin ? JSON.stringify(command.raw_origin) : null,
				run_id: command.run_id ?? null,
				schema_version: command.schema_version,
				sent_at: command.sent_at,
				status: "accepted",
				thread_id: command.thread_id,
			})
			.pipe(Effect.asVoid);

	const read_existing_command = (transaction: GraphTransaction, command: CommandEnvelope) =>
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

			if (!existing) {
				return undefined;
			}

			if (!command_matches(command, existing)) {
				return yield* new AgentGraphCommandConflict({ message_id: command.message_id });
			}

			const [claim] = yield* transaction
				.select()
				.from(OrchestrationGraphCommands)
				.where(eq(OrchestrationGraphCommands.message_id, command.message_id))
				.limit(1);

			if (!claim) {
				return yield* new AgentGraphCommandConflict({ message_id: command.message_id });
			}

			return claim;
		});

	const command_acceptance = (
		group_id: string,
		events: ReadonlyArray<EventEnvelope>,
		status: "accepted" | "duplicate",
	) => {
		const journal_sequence = events.at(-1)?.journal_sequence;

		return journal_sequence === undefined
			? Effect.fail(
					new AgentGraphInvalid({ message: "Graph command has no correlated event" }),
				)
			: Effect.succeed({ events, group_id, journal_sequence, status });
	};

	return {
		append_event,
		command_acceptance,
		insert_journal_command,
		publish_events,
		read_correlated_events,
		read_existing_command,
	};
}
