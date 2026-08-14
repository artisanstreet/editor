import { Context, Effect, Layer } from "effect";

import type { CommandEnvelope, CommandReceiptEnvelope, OutboundEnvelope } from "@artisan/protocol";

import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import type { AgentGraphError } from "../orchestration/agent-graph-repository";
import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import type { OrchestrationError } from "../persistence/orchestration/repository";
import type { JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/metadata";
import { RuntimeCatalogService, type RuntimeCatalogPolicyError } from "../runtime/catalog";
import { ThreadCommands } from "../threads/thread-commands";
import type { ThreadMetadataError } from "../threads/thread-metadata-repository";
import type { ThreadProjectAffinityError } from "../threads/thread-project-affinity-repository";
import { TerminalSessionService, type TerminalSessionError } from "../terminal/sessions";

export class CommandRouter extends Context.Service<
	CommandRouter,
	{
		readonly Dispatch: (
			command: CommandEnvelope,
		) => Effect.Effect<
			ReadonlyArray<OutboundEnvelope>,
			| AgentGraphError
			| JournalStoreError
			| OrchestrationError
			| RuntimeCatalogPolicyError
			| TerminalSessionError
			| ThreadMetadataError
			| ThreadProjectAffinityError
		>;
	}
>()("Artisan/CommandRouter") {}

export const CommandRouterLive = Layer.effect(
	CommandRouter,
	Effect.gen(function* () {
		const graph = yield* AgentGraphOrchestrator;
		const orchestrator = yield* AgentOrchestrator;
		const metadata = yield* RuntimeMetadata;
		const runtime_catalog = yield* RuntimeCatalogService;
		const thread_commands = yield* ThreadCommands;
		const terminals = yield* TerminalSessionService;
		const HandleOrchestration = (command: CommandEnvelope) =>
			command.payload.type === "thread.session_policy.update"
				? runtime_catalog
						.ValidateThreadSessionPolicy(command.payload.policy)
						.pipe(Effect.andThen(orchestrator.HandleInbound(command)))
				: orchestrator.HandleInbound(command);

		/**
		 * Every accepted command answers with the same receipt envelope; the
		 * only variation between acceptance paths is which run identity rides
		 * along, so it is the single parameter.
		 */
		const AcceptedReceipt = (
			command: CommandEnvelope,
			accepted: {
				readonly events: ReadonlyArray<OutboundEnvelope>;
				readonly journal_sequence: number;
				readonly status: "accepted" | "duplicate";
			},
			run_id: string | undefined,
		) =>
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
					...(run_id === undefined ? {} : { run_id }),
					schema_version: 1,
					sent_at,
					thread_id: command.thread_id,
				};

				return [receipt, ...accepted.events];
			});
		const Dispatch = (command: CommandEnvelope) => {
			if (command.payload.type === "usage.interruption.resolve") {
				return orchestrator
					.HandleUsageInterruption(command)
					.pipe(
						Effect.flatMap((accepted) =>
							AcceptedReceipt(command, accepted, accepted.run_id),
						),
					);
			}

			return command.payload.type === "thread.create"
				? thread_commands.HandleCreate(command)
				: command.payload.type === "thread.retention.update"
					? thread_commands.HandleRetentionPolicy(command)
					: command.payload.type === "model.favorite.update"
						? thread_commands.HandleModelFavorite(command)
						: command.payload.type === "session.defaults.update"
							? thread_commands.HandleSessionDefaults(command)
							: command.payload.type === "thread.project.assign" ||
								  command.payload.type === "thread.project.unlock"
								? thread_commands.HandleProjectAffinity(command)
								: command.payload.type.startsWith("thread.") &&
									  command.payload.type !== "thread.send_message" &&
									  command.payload.type !== "thread.auto_steer.update" &&
									  command.payload.type !== "thread.session_policy.update"
									? thread_commands.HandleMetadata(command)
									: command.payload.type.startsWith("terminal.")
										? terminals
												.Handle(command)
												.pipe(
													Effect.flatMap((accepted) =>
														AcceptedReceipt(
															command,
															accepted,
															command.run_id,
														),
													),
												)
										: command.payload.type === "orchestration.group.start" ||
											  command.payload.type === "agent_instance.rename" ||
											  command.payload.type.startsWith("assignment.")
											? graph
													.Handle(command)
													.pipe(
														Effect.flatMap((accepted) =>
															AcceptedReceipt(
																command,
																accepted,
																undefined,
															),
														),
													)
											: HandleOrchestration(command).pipe(
													Effect.flatMap((accepted) =>
														AcceptedReceipt(
															command,
															accepted,
															accepted.run_id,
														),
													),
												);
		};

		return {
			Dispatch,
		};
	}),
);
