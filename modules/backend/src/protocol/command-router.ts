import { Context, Effect, Layer } from "effect";

import type { CommandEnvelope, CommandReceiptEnvelope, OutboundEnvelope } from "@artisan/protocol";

import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import type { OrchestrationError } from "../persistence/orchestration-repository";
import type { JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { ThreadCommands } from "../threads/thread-commands";

export class CommandRouter extends Context.Service<
	CommandRouter,
	{
		readonly Dispatch: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError | OrchestrationError>;
	}
>()("Artisan/CommandRouter") {}

export const CommandRouterLive = Layer.effect(
	CommandRouter,
	Effect.gen(function* () {
		const orchestrator = yield* AgentOrchestrator;
		const metadata = yield* RuntimeMetadata;
		const thread_commands = yield* ThreadCommands;
		const Dispatch = (command: CommandEnvelope) =>
			command.payload.type === "thread.create"
				? thread_commands.HandleCreate(command)
				: orchestrator.Handle(command).pipe(
						Effect.flatMap((accepted) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								const receipt: CommandReceiptEnvelope = {
									causation_id: command.message_id,
									correlation_id: command.message_id,
									kind: "command.receipt",
									message_id,
									origin: "backend",
									payload: {
										journal_sequence: accepted.journal_sequence,
										status: accepted.status,
									},
									protocol_version: 1,
									...(command.agent_id ? { agent_id: command.agent_id } : {}),
									run_id: accepted.run_id,
									schema_version: 1,
									sent_at,
									thread_id: command.thread_id,
								};

								return [receipt, ...accepted.events];
							}),
						),
					);

		return {
			Dispatch,
		};
	}),
);
