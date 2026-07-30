import { Context, Effect, Layer } from "effect";

import type {
	CommandEnvelope,
	CommandReceiptEnvelope,
	EventEnvelope,
	OutboundEnvelope,
} from "@artisan/protocol";

import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/metadata";
import { ThreadMetadataRepository, type ThreadMetadataError } from "./thread-metadata-repository";
import {
	ThreadProjectAffinityRepository,
	type ThreadProjectAffinityError,
} from "./thread-project-affinity-repository";
import { ModelFavoritesService } from "../model-favorites/service";
import { SessionDefaultsService } from "../settings/session-defaults-service";
import { ThreadRetentionPolicyService } from "./thread-retention-policy";

export class ThreadCommands extends Context.Service<
	ThreadCommands,
	{
		readonly HandleCreate: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
		readonly HandleMetadata: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, ThreadMetadataError>;
		readonly HandleRetentionPolicy: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
		readonly HandleModelFavorite: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
		readonly HandleSessionDefaults: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
		readonly HandleProjectAffinity: (
			command: CommandEnvelope,
		) => Effect.Effect<
			ReadonlyArray<OutboundEnvelope>,
			JournalStoreError | ThreadProjectAffinityError
		>;
	}
>()("Artisan/ThreadCommands") {}

export const ThreadCommandsLive = Layer.effect(
	ThreadCommands,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const repository = yield* ThreadMetadataRepository;
		const project_affinity = yield* ThreadProjectAffinityRepository;
		const retention_policy = yield* ThreadRetentionPolicyService;
		const model_favorites = yield* ModelFavoritesService;
		const session_defaults = yield* SessionDefaultsService;

		const MakeOutput = (
			command: CommandEnvelope,
			status: "accepted" | "duplicate",
			event: EventEnvelope,
		) =>
			Effect.gen(function* () {
				const receipt_id = yield* metadata.MakeId("message");
				const receipt_time = yield* metadata.Now;
				const receipt: CommandReceiptEnvelope = {
					causation_id: command.message_id,
					correlation_id: command.message_id,
					kind: "command.receipt",
					message_id: receipt_id,
					origin: "backend",
					payload: { journal_sequence: event.journal_sequence, status },
					protocol_version: 1,
					schema_version: 1,
					sent_at: receipt_time,
					thread_id: command.thread_id,
					...(command.agent_id ? { agent_id: command.agent_id } : {}),
					...(command.run_id ? { run_id: command.run_id } : {}),
				};

				return [receipt, event];
			});

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
		const HandleMetadata = (command: CommandEnvelope) =>
			repository
				.Accept(command)
				.pipe(
					Effect.flatMap((accepted) =>
						MakeOutput(command, accepted.status, accepted.event),
					),
				);
		const HandleRetentionPolicy = (command: CommandEnvelope) =>
			retention_policy
				.Update(command)
				.pipe(
					Effect.flatMap((accepted) =>
						MakeOutput(command, accepted.status, accepted.event),
					),
				);
		const HandleModelFavorite = (command: CommandEnvelope) =>
			model_favorites
				.Update(command)
				.pipe(
					Effect.flatMap((accepted) =>
						MakeOutput(command, accepted.status, accepted.event),
					),
				);
		const HandleSessionDefaults = (command: CommandEnvelope) =>
			session_defaults
				.Update(command)
				.pipe(
					Effect.flatMap((accepted) =>
						MakeOutput(command, accepted.status, accepted.event),
					),
				);
		const HandleProjectAffinity = (command: CommandEnvelope) =>
			project_affinity
				.Accept(command)
				.pipe(
					Effect.flatMap((accepted) =>
						MakeOutput(command, accepted.status, accepted.event),
					),
				);

		return {
			HandleCreate,
			HandleMetadata,
			HandleModelFavorite,
			HandleSessionDefaults,
			HandleProjectAffinity,
			HandleRetentionPolicy,
		};
	}),
);
