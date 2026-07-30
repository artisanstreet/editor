import { eq } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import {
	type CommandEnvelope,
	type EventEnvelope,
	type TerminalLifecycleEvent,
	type TerminalSession,
} from "@artisan/protocol";

import type { DatabaseClient } from "../persistence/database";
import { EventStreams, JournalEvents } from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";
import { RecordThreadActivity } from "../threads/internal/thread-activity";
import type { TerminalLifecycleAction } from "./model";

interface TerminalEventInput {
	readonly action: TerminalLifecycleAction;
	readonly agent_id?: string;
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly raw_origin?: {
		readonly provider: string;
		readonly reference: string;
	};
	readonly run_id?: string;
	readonly terminal: TerminalSession;
}

type TerminalTransaction = DatabaseClient;

export class TerminalJournal extends Context.Service<
	TerminalJournal,
	{
		readonly Append: (
			transaction: TerminalTransaction,
			input: TerminalEventInput,
		) => Effect.Effect<EventEnvelope, unknown>;
	}
>()("Artisan/TerminalJournal") {}

export const CommandEventInput = (
	command: CommandEnvelope,
	action: TerminalLifecycleAction,
	terminal: TerminalSession,
): TerminalEventInput => ({
	...(command.agent_id ? { agent_id: command.agent_id } : {}),
	action,
	causation_id: command.message_id,
	correlation_id: command.message_id,
	...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
	...(command.run_id ? { run_id: command.run_id } : {}),
	terminal,
});

export const TerminalJournalLive = Layer.effect(
	TerminalJournal,
	Effect.gen(function* () {
		const metadata = yield* RuntimeMetadata;

		const Append = (transaction: TerminalTransaction, input: TerminalEventInput) =>
			Effect.gen(function* () {
				const stream_id = `thread:${input.terminal.thread_id}`;
				const [stream] = yield* transaction
					.select({ last_sequence: EventStreams.last_sequence })
					.from(EventStreams)
					.where(eq(EventStreams.stream_id, stream_id))
					.limit(1);
				const sequence = (stream?.last_sequence ?? 0) + 1;
				const event_id = yield* metadata.MakeId("event");
				const occurred_at = yield* metadata.Now;
				const payload = {
					action: input.action,
					terminal: input.terminal,
					type: "terminal.lifecycle" as const,
				} satisfies TerminalLifecycleEvent;

				yield* RecordThreadActivity(
					transaction,
					input.terminal.thread_id,
					occurred_at,
					payload,
				);

				yield* stream
					? transaction
							.update(EventStreams)
							.set({ last_sequence: sequence })
							.where(eq(EventStreams.stream_id, stream_id))
					: transaction.insert(EventStreams).values({
							last_sequence: sequence,
							stream_id,
						});

				const [inserted] = yield* transaction
					.insert(JournalEvents)
					.values({
						agent_id: input.agent_id ?? null,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						event_id,
						event_type: payload.type,
						occurred_at,
						origin: "backend",
						payload_json: JSON.stringify(payload),
						raw_origin_json: input.raw_origin ? JSON.stringify(input.raw_origin) : null,
						run_id: input.run_id ?? null,
						schema_version: 1,
						stream_id,
						stream_sequence: sequence,
						thread_id: input.terminal.thread_id,
					})
					.returning({ journal_sequence: JournalEvents.sequence });

				if (!inserted) {
					return yield* Effect.die(new Error("Terminal journal insert returned no row"));
				}

				return {
					...(input.agent_id ? { agent_id: input.agent_id } : {}),
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					journal_sequence: inserted.journal_sequence,
					kind: "event",
					message_id: event_id,
					origin: "backend",
					payload,
					protocol_version: 1,
					...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
					...(input.run_id ? { run_id: input.run_id } : {}),
					schema_version: 1,
					sequence,
					sent_at: occurred_at,
					stream_id,
					thread_id: input.terminal.thread_id,
				} satisfies EventEnvelope;
			});

		return { Append };
	}),
);
