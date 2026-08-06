import { Context, Data, Effect } from "effect";

import {
	type CommandEnvelope,
	type EventEnvelope,
	type ThreadMessageRoutedEvent,
	type ThreadSessionPolicy,
	type ThreadSessionSnapshot,
	type ThreadWorkItem,
} from "@artisan/protocol";
import type { EngineObservation, EngineResumeToken } from "@artisan/engines";

import type { IntakeAssessment } from "../../orchestration/intake-policy";
import type {
	AuthoritativeCommandEnvelope,
	AuthoritativeThreadSendMessageCommand,
} from "./message-command";

export type WorkStatus = ThreadWorkItem["status"];
export type OutboxKind =
	| "start"
	| "steer"
	| "cancel"
	| "close"
	| "respond_approval"
	| "respond_question";

export class OrchestrationCommandConflict extends Data.TaggedError("OrchestrationCommandConflict")<{
	readonly message_id: string;
}> {}

export class OrchestrationNotFound extends Data.TaggedError("OrchestrationNotFound")<{
	readonly resource: "project" | "run" | "thread";
	readonly id: string;
}> {}

/** Rejects message execution when its thread has no live Forge-owned project authority. */
export class OrchestrationProjectAuthorityError extends Data.TaggedError(
	"OrchestrationProjectAuthorityError",
)<{
	readonly project_id?: string;
	readonly reason: "project_detached" | "thread_unassigned";
	readonly thread_id: string;
}> {}

export class OrchestrationFailure extends Data.TaggedError("OrchestrationFailure")<{
	readonly cause: unknown;
}> {}

export type OrchestrationError =
	| OrchestrationCommandConflict
	| OrchestrationFailure
	| OrchestrationNotFound
	| OrchestrationProjectAuthorityError;

export interface AcceptedOrchestrationCommand {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly journal_sequence: number;
	readonly run_id: string;
	readonly status: "accepted" | "duplicate";
}

export interface PendingWork {
	readonly agent_id: string;
	readonly command_id: string;
	readonly engine_id: string;
	readonly kind: OutboxKind;
	readonly payload:
		| (CommandEnvelope["payload"] & {
				readonly type: Exclude<CommandEnvelope["payload"]["type"], "thread.send_message">;
		  })
		| AuthoritativeThreadSendMessageCommand;
	readonly run_id: string;
	readonly thread_id: string;
	readonly working_directory: string;
}

/** A durably interrupted run whose native engine session can be reopened without starting work again. */
export interface RecoverableNativeRun {
	readonly agent_id: string;
	readonly engine_id: string;
	readonly resume_token: EngineResumeToken;
	readonly run_id: string;
	readonly thread_id: string;
	readonly working_directory: string;
}

export class OrchestrationRepository extends Context.Service<
	OrchestrationRepository,
	{
		readonly Accept: (
			command: AuthoritativeCommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly AcceptInbound: (
			command: CommandEnvelope,
			can_steer: boolean,
			intake?: IntakeAssessment,
			routing_reason?: ThreadMessageRoutedEvent["reason"],
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly CompleteOutbox: (command_id: string) => Effect.Effect<void, OrchestrationError>;
		readonly ClaimOutbox: (command_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly ClaimNativeRecoveries: () => Effect.Effect<
			ReadonlyArray<RecoverableNativeRun>,
			OrchestrationError
		>;
		readonly FallbackSteering: (
			command_id: string,
			reason?: "delivery_failed" | "rejected",
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly GetPending: () => Effect.Effect<ReadonlyArray<PendingWork>, OrchestrationError>;
		readonly GetWork: (
			thread_id: string,
		) => Effect.Effect<ThreadWorkItem | undefined, OrchestrationError>;
		readonly GetAutoSteer: (thread_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly GetSessionPolicy: (
			thread_id: string,
		) => Effect.Effect<ThreadSessionPolicy, OrchestrationError>;
		readonly GetSession: (
			thread_id: string,
		) => Effect.Effect<ThreadSessionSnapshot, OrchestrationError>;
		readonly MarkInterrupted: () => Effect.Effect<void, OrchestrationError>;
		readonly MarkOutboxUndeliverable: (
			command_id: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly MarkRunStarted: (
			run_id: string,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly MarkRunResumed: (run_id: string) => Effect.Effect<boolean, OrchestrationError>;
		readonly PersistNativeRun: (
			run_id: string,
			native_thread_id: string,
			resume_token: unknown,
			model_id?: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly RecordObservation: (
			observation: EngineObservation,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
		readonly RecordObservations: (
			observations: ReadonlyArray<EngineObservation>,
		) => Effect.Effect<ReadonlyArray<EventEnvelope>, OrchestrationError>;
	}
>()("Artisan/OrchestrationRepository") {}
