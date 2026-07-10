import { Context, Effect, Layer, Match, pipe } from "effect";

import {
	DecodeCommandEnvelope,
	type CommandEnvelope,
	type CommandReceiptEnvelope,
	type OutboundEnvelope,
	type ProtocolErrorDetail,
	type ProtocolErrorEnvelope,
} from "@artisan/protocol";

import { type JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { CommandRouter } from "./command-router";

const describe_journal_error = pipe(
	Match.type<JournalStoreError>(),
	Match.tagsExhaustive({
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
		readonly Route: (input: unknown) => Effect.Effect<ReadonlyArray<OutboundEnvelope>>;
	}
>()("Artisan/ProtocolRouter") {}

export const ProtocolRouterLive = Layer.effect(
	ProtocolRouter,
	Effect.gen(function* () {
		const commands = yield* CommandRouter;
		const metadata = yield* RuntimeMetadata;

		const MakeRejectedReceipt = (command: CommandEnvelope, error: JournalStoreError) =>
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

		const Route = (input: unknown) =>
			DecodeCommandEnvelope(input).pipe(
				Effect.flatMap((command) =>
					commands
						.Dispatch(command)
						.pipe(Effect.catch((error) => MakeRejectedReceipt(command, error))),
				),
				Effect.catch(() => MakeProtocolError),
			);

		return { Route };
	}),
);
