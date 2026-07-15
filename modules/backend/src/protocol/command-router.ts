import { Context, Effect, Layer, Option } from "effect";

import type { CommandEnvelope, CommandReceiptEnvelope, OutboundEnvelope } from "@artisan/protocol";

import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import type { AgentGraphError } from "../orchestration/agent-graph-repository";
import { AgentOrchestrator } from "../orchestration/agent-orchestrator";
import type { OrchestrationError } from "../persistence/orchestration-repository";
import type { JournalStoreError } from "../persistence/journal-store";
import {
	PreviewBrowserLifecycle,
	type PreviewBrowserLifecycleError,
} from "../preview/preview-browser";
import {
	PreviewTarget,
	type PreviewTargetAcceptance,
	type PreviewTargetError,
	type PreviewTargetRemovalReplay,
} from "../preview/preview-target";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { ThreadCommands } from "../threads/thread-commands";
import type { ThreadMetadataError } from "../threads/thread-metadata-repository";
import type { ThreadProjectAffinityError } from "../threads/thread-project-affinity-repository";
import { TerminalSessionService, type TerminalSessionError } from "../terminal/terminal-sessions";

type PreviewTargetRemovePayload = Extract<
	CommandEnvelope["payload"],
	{ readonly type: "preview.target.remove" }
>;

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
			| PreviewBrowserLifecycleError
			| PreviewTargetError
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
		const preview_browser = yield* PreviewBrowserLifecycle;
		const preview_targets = yield* PreviewTarget;
		const thread_commands = yield* ThreadCommands;
		const terminals = yield* TerminalSessionService;
		const SettleRemovalReplay = (
			command: CommandEnvelope,
			replay: PreviewTargetRemovalReplay,
		) =>
			replay.fence_status === "complete"
				? Effect.succeed(replay)
				: preview_browser
						.SettleTargetRemovalFence(command.message_id)
						.pipe(Effect.as(replay));
		const ReplayRemovalAfterFailure = (
			command: CommandEnvelope,
			failure: PreviewBrowserLifecycleError | PreviewTargetError,
		) =>
			preview_targets.ReplayRemoval(command).pipe(
				Effect.flatMap(
					Option.match({
						onNone: () => Effect.fail(failure),
						onSome: (replay) => SettleRemovalReplay(command, replay),
					}),
				),
			);
		const RemovePreviewTarget = (
			command: CommandEnvelope,
			payload: PreviewTargetRemovePayload,
		) =>
			preview_targets.ReplayRemoval(command).pipe(
				Effect.flatMap(
					Option.match({
						onNone: () =>
							preview_browser
								.SynchronizeTargetRemoval(payload, (claim) =>
									preview_targets.RemoveClaimed(command, claim),
								)
								.pipe(
									Effect.flatMap((replay) =>
										SettleRemovalReplay(command, replay),
									),
									Effect.catch((error) =>
										ReplayRemovalAfterFailure(command, error),
									),
								),
						onSome: (replay) => SettleRemovalReplay(command, replay),
					}),
				),
			);
		const Dispatch = (command: CommandEnvelope) =>
			command.payload.type === "thread.create"
				? thread_commands.HandleCreate(command)
				: command.payload.type === "thread.retention.update"
					? thread_commands.HandleRetentionPolicy(command)
					: command.payload.type === "thread.project.assign" ||
						  command.payload.type === "thread.project.unlock"
						? thread_commands.HandleProjectAffinity(command)
						: command.payload.type.startsWith("thread.") &&
							  command.payload.type !== "thread.send_message"
							? thread_commands.HandleMetadata(command)
							: command.payload.type.startsWith("terminal.")
								? terminals.Handle(command).pipe(
										Effect.flatMap((accepted) =>
											Effect.gen(function* () {
												const message_id =
													yield* metadata.MakeId("message");
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
													...(command.agent_id
														? { agent_id: command.agent_id }
														: {}),
													...(command.run_id
														? { run_id: command.run_id }
														: {}),
													schema_version: 1,
													sent_at,
													thread_id: command.thread_id,
												};

												return [receipt, ...accepted.events];
											}),
										),
									)
								: command.payload.type === "preview.browser.open" ||
									  command.payload.type === "preview.inspection.attach" ||
									  command.payload.type === "preview.inspection.detach"
									? (command.payload.type === "preview.browser.open"
											? preview_browser.Open(command)
											: command.payload.type === "preview.inspection.attach"
												? preview_browser.Attach(command)
												: preview_browser.Detach(command)
										).pipe(
											Effect.flatMap((accepted) =>
												Effect.gen(function* () {
													const message_id =
														yield* metadata.MakeId("message");
													const sent_at = yield* metadata.Now;
													const receipt: CommandReceiptEnvelope = {
														causation_id: command.message_id,
														correlation_id: command.message_id,
														kind: "command.receipt",
														message_id,
														origin: "backend",
														payload: {
															journal_sequence:
																accepted.event.journal_sequence,
															status: accepted.status,
														},
														protocol_version: 1,
														...(command.agent_id
															? { agent_id: command.agent_id }
															: {}),
														...(command.run_id
															? { run_id: command.run_id }
															: {}),
														schema_version: 1,
														sent_at,
														thread_id: command.thread_id,
													};

													return [receipt, accepted.event];
												}),
											),
										)
									: command.payload.type === "preview.target.register" ||
										  command.payload.type === "preview.target.probe" ||
										  command.payload.type === "preview.target.remove"
										? (command.payload.type === "preview.target.register"
												? preview_targets.Register(command)
												: command.payload.type === "preview.target.probe"
													? Effect.scoped(preview_targets.Probe(command))
													: RemovePreviewTarget(
															command,
															command.payload,
														).pipe(
															Effect.map(
																(
																	replay,
																): PreviewTargetAcceptance => ({
																	event: replay.event,
																	status: replay.status,
																}),
															),
														)
											).pipe(
												Effect.flatMap((accepted) =>
													Effect.gen(function* () {
														const message_id =
															yield* metadata.MakeId("message");
														const sent_at = yield* metadata.Now;

														const receipt: CommandReceiptEnvelope = {
															causation_id: command.message_id,
															correlation_id: command.message_id,
															kind: "command.receipt",
															message_id,
															origin: "backend",
															payload: {
																journal_sequence:
																	accepted.event.journal_sequence,
																status: accepted.status,
															},
															protocol_version: 1,
															...(command.agent_id
																? { agent_id: command.agent_id }
																: {}),
															...(command.run_id
																? { run_id: command.run_id }
																: {}),
															schema_version: 1,
															sent_at,
															thread_id: command.thread_id,
														};

														return [receipt, accepted.event];
													}),
												),
											)
										: command.payload.type === "orchestration.group.start" ||
											  command.payload.type === "agent_instance.rename" ||
											  command.payload.type.startsWith("assignment.")
											? graph.Handle(command).pipe(
													Effect.flatMap((accepted) =>
														Effect.gen(function* () {
															const message_id =
																yield* metadata.MakeId("message");
															const sent_at = yield* metadata.Now;
															const receipt: CommandReceiptEnvelope =
																{
																	causation_id:
																		command.message_id,
																	correlation_id:
																		command.message_id,
																	kind: "command.receipt",
																	message_id,
																	origin: "backend",
																	payload: {
																		journal_sequence:
																			accepted.journal_sequence,
																		status: accepted.status,
																	},
																	protocol_version: 1,
																	...(command.agent_id
																		? {
																				agent_id:
																					command.agent_id,
																			}
																		: {}),
																	schema_version: 1,
																	sent_at,
																	thread_id: command.thread_id,
																};

															return [receipt, ...accepted.events];
														}),
													),
												)
											: orchestrator.Handle(command).pipe(
													Effect.flatMap((accepted) =>
														Effect.gen(function* () {
															const message_id =
																yield* metadata.MakeId("message");
															const sent_at = yield* metadata.Now;
															const receipt: CommandReceiptEnvelope =
																{
																	causation_id:
																		command.message_id,
																	correlation_id:
																		command.message_id,
																	kind: "command.receipt",
																	message_id,
																	origin: "backend",
																	payload: {
																		journal_sequence:
																			accepted.journal_sequence,
																		status: accepted.status,
																	},
																	protocol_version: 1,
																	...(command.agent_id
																		? {
																				agent_id:
																					command.agent_id,
																			}
																		: {}),
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
