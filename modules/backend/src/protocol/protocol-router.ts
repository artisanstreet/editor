import { Context, Effect, Layer, Match, pipe } from "effect";

import {
	DecodeInboundControlEnvelope,
	type CommandEnvelope,
	type CommandReceiptEnvelope,
	type InboundControlEnvelope,
	type OutboundEnvelope,
	type ProtocolErrorDetail,
	type ProtocolErrorEnvelope,
} from "@artisan/protocol";

import { type AgentGraphError } from "../orchestration/agent-graph-repository";
import { type JournalStoreError } from "../persistence/journal-store";
import { type OrchestrationError } from "../persistence/orchestration-repository";
import type { TerminalSessionError } from "../terminal/terminal-sessions";
import type { ThreadMetadataError } from "../threads/thread-metadata-repository";
import type { ThreadProjectAffinityError } from "../threads/thread-project-affinity-repository";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { CommandRouter } from "./command-router";

/**
 * Identifies the connection-level frames whose lifecycle, subscription, and
 * query semantics belong to `ProtocolServer`, rather than the command-domain
 * router. Keeping this explicit prevents a command-only router from being
 * mistaken for the complete control-envelope router.
 */
export type ProtocolRouterInboundDispatch =
	| {
			readonly _tag: "Command";
			readonly command: CommandEnvelope;
	  }
	| {
			readonly _tag: "Connection";
			readonly envelope: Exclude<InboundControlEnvelope, CommandEnvelope>;
	  };

const describe_journal_error = pipe(
	Match.type<
		| AgentGraphError
		| JournalStoreError
		| OrchestrationError
		| TerminalSessionError
		| ThreadMetadataError
		| ThreadProjectAffinityError
	>(),
	Match.tagsExhaustive({
		AgentGraphCommandConflict: (): ProtocolErrorDetail => ({
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		}),
		AgentGraphNotFound: (error): ProtocolErrorDetail => ({
			code: `${error.resource}.not_found`,
			message: `The requested ${error.resource} does not exist.`,
			retryable: false,
		}),
		AgentGraphInvalid: (error): ProtocolErrorDetail => ({
			code: "orchestration.graph_invalid",
			message: error.message,
			retryable: false,
		}),
		AgentGraphFailure: (): ProtocolErrorDetail => ({
			code: "orchestration.unavailable",
			message: "The graph command could not be durably orchestrated.",
			retryable: true,
		}),
		OrchestrationCommandConflict: (): ProtocolErrorDetail => ({
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		}),
		OrchestrationNotFound: (error): ProtocolErrorDetail => ({
			code: `${error.resource}.not_found`,
			message: `The requested ${error.resource} does not exist.`,
			retryable: false,
		}),
		OrchestrationFailure: (): ProtocolErrorDetail => ({
			code: "orchestration.unavailable",
			message: "The command could not be durably orchestrated.",
			retryable: true,
		}),
		TerminalCommandConflict: (): ProtocolErrorDetail => ({
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		}),
		TerminalNotFound: (): ProtocolErrorDetail => ({
			code: "terminal.not_found",
			message: "The requested terminal does not exist.",
			retryable: false,
		}),
		TerminalNotActive: (): ProtocolErrorDetail => ({
			code: "terminal.not_active",
			message: "The requested terminal is not active.",
			retryable: false,
		}),
		TerminalInvariantError: (): ProtocolErrorDetail => ({
			code: "terminal.invariant_failed",
			message: "The stored terminal state is invalid.",
			retryable: false,
		}),
		TerminalPersistenceFailure: (): ProtocolErrorDetail => ({
			code: "terminal.unavailable",
			message: "The terminal command could not be durably completed.",
			retryable: true,
		}),
		CommandIdConflict: (): ProtocolErrorDetail => ({
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		}),
		ThreadAlreadyExists: (error): ProtocolErrorDetail => ({
			code: "thread.already_exists",
			message: `Thread ${error.thread_id} already exists.`,
			retryable: false,
		}),
		ThreadReservedScope: (): ProtocolErrorDetail => ({
			code: "thread.reserved_scope",
			message: "This thread id belongs to an internal Artisan settings scope.",
			retryable: false,
		}),
		ThreadNotFound: (error): ProtocolErrorDetail => ({
			code: "thread.not_found",
			message: `Thread ${error.thread_id} does not exist.`,
			retryable: false,
		}),
		ThreadProjectAffinityNotFound: (error): ProtocolErrorDetail => ({
			code: "thread.not_found",
			message: `Thread ${error.thread_id} does not exist.`,
			retryable: false,
		}),
		JournalInvariantError: (): ProtocolErrorDetail => ({
			code: "journal.invariant_failed",
			message: "The journal could not reconstruct the accepted command.",
			retryable: false,
		}),
		JournalStoreFailure: (): ProtocolErrorDetail => ({
			code: "journal.unavailable",
			message: "The command could not be durably accepted.",
			retryable: true,
		}),
	}),
);

export class ProtocolRouter extends Context.Service<
	ProtocolRouter,
	{
		/** Decodes every inbound control envelope and exposes its owning routing boundary. */
		readonly ClassifyInbound: (
			input: unknown,
		) => Effect.Effect<ProtocolRouterInboundDispatch, unknown>;
		/** Routes one already-classified durable command through its owning domain router. */
		readonly RouteCommand: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>>;
		/**
		 * Preserves the original command-only convenience boundary. Connection-owned
		 * frames return the same uncorrelated protocol error as malformed input.
		 */
		readonly Route: (input: unknown) => Effect.Effect<ReadonlyArray<OutboundEnvelope>>;
	}
>()("Artisan/ProtocolRouter") {}

export const ProtocolRouterLive = Layer.effect(
	ProtocolRouter,
	Effect.gen(function* () {
		const commands = yield* CommandRouter;
		const metadata = yield* RuntimeMetadata;

		const MakeRejectedReceipt = (
			command: CommandEnvelope,
			error:
				| AgentGraphError
				| JournalStoreError
				| OrchestrationError
				| TerminalSessionError
				| ThreadMetadataError
				| ThreadProjectAffinityError,
		) =>
			Effect.gen(function* () {
				const message_id = yield* metadata.MakeId("message");
				const sent_at = yield* metadata.Now;

				const receipt: CommandReceiptEnvelope = {
					protocol_version: 1,
					schema_version: 1,
					kind: "command.receipt",
					message_id,
					correlation_id: command.message_id,
					thread_id: command.thread_id,
					...(command.run_id ? { run_id: command.run_id } : {}),
					...(command.agent_id ? { agent_id: command.agent_id } : {}),
					causation_id: command.message_id,
					origin: "backend",
					sent_at,
					payload: {
						status: "rejected",
						error: describe_journal_error(error),
					},
				};

				return [receipt];
			});

		const MakeProtocolError = Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			const error: ProtocolErrorEnvelope = {
				protocol_version: 1,
				schema_version: 1,
				kind: "protocol.error",
				message_id,
				origin: "backend",
				sent_at,
				payload: {
					code: "protocol.invalid_message",
					message: "The message does not match the Artisan protocol.",
					retryable: false,
				},
			};

			return [error];
		});

		const ClassifyInbound = (input: unknown) =>
			DecodeInboundControlEnvelope(input).pipe(
				Effect.map((envelope): ProtocolRouterInboundDispatch => {
					if (envelope.kind === "command") {
						return { _tag: "Command", command: envelope };
					}

					return { _tag: "Connection", envelope };
				}),
			);

		const RouteCommand = (command: CommandEnvelope) =>
			commands
				.Dispatch(command)
				.pipe(Effect.catch((error) => MakeRejectedReceipt(command, error)));

		const Route = (input: unknown) =>
			ClassifyInbound(input).pipe(
				Effect.flatMap((dispatch) =>
					dispatch._tag === "Command"
						? RouteCommand(dispatch.command)
						: MakeProtocolError,
				),
				Effect.catch(() => MakeProtocolError),
			);

		return { ClassifyInbound, Route, RouteCommand };
	}),
);
