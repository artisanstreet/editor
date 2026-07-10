import { Context, Effect, Layer } from "effect";

import type {
	CommandEnvelope,
	CommandReceiptEnvelope,
	EventEnvelope,
	OutboundEnvelope,
} from "@artisan/protocol";

import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";

export class ThreadCommands extends Context.Service<
	ThreadCommands,
	{
		readonly HandleCreate: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
	}
>()("Artisan/ThreadCommands") {}

export const ThreadCommandsLive = Layer.effect(
	ThreadCommands,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;

		const HandleCreate = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				const acceptance = yield* journal.AcceptThreadCreate(command);

				const receipt_id = yield* metadata.MakeId("message");
				const receipt_time = yield* metadata.Now;

				const receipt: CommandReceiptEnvelope = {
					protocol_version: 1,
					schema_version: 1,
					kind: "command.receipt",
					message_id: receipt_id,
					correlation_id: command.message_id,
					thread_id: acceptance.thread_id,
					...(acceptance.run_id ? { run_id: acceptance.run_id } : {}),
					...(acceptance.agent_id ? { agent_id: acceptance.agent_id } : {}),
					causation_id: command.message_id,
					origin: "backend",
					sent_at: receipt_time,
					payload: {
						journal_sequence: acceptance.journal_sequence,
						status: acceptance.status,
					},
				};

				const event: EventEnvelope = {
					protocol_version: 1,
					schema_version: 1,
					kind: "event",
					message_id: acceptance.event_id,
					correlation_id: command.message_id,
					causation_id: command.message_id,
					stream_id: acceptance.stream_id,
					sequence: acceptance.sequence,
					journal_sequence: acceptance.journal_sequence,
					thread_id: acceptance.thread_id,
					...(acceptance.run_id ? { run_id: acceptance.run_id } : {}),
					...(acceptance.agent_id ? { agent_id: acceptance.agent_id } : {}),
					origin: "backend",
					...(acceptance.raw_origin ? { raw_origin: acceptance.raw_origin } : {}),
					sent_at: acceptance.occurred_at,
					payload: acceptance.payload,
				};

				return [receipt, event];
			});

		return { HandleCreate };
	}),
);
