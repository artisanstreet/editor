import { Data, Effect } from "effect";

import type { EngineObservation, EngineRunTerminalState } from "@artisan/engines";
import type {
	AgentRun,
	AssignmentControlEvent,
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	CommandEnvelope,
	EventEnvelope,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	OrchestrationGroupSummary,
} from "@artisan/protocol";

export type AgentGraphCommand = Extract<
	CommandEnvelope["payload"],
	{
		readonly type:
			| "agent_instance.rename"
			| "assignment.heartbeat"
			| "assignment.pause"
			| "assignment.resume"
			| "assignment.retry"
			| "assignment.steer"
			| "assignment.stop"
			| "orchestration.group.start";
	}
>;

export type AgentGraphControlAction = AssignmentControlEvent["action"];
export type AgentGraphControlOutcome = AssignmentControlEvent["outcome"];

/** Reports reuse of a graph command identifier with changed intent. */
export class AgentGraphCommandConflict extends Data.TaggedError("AgentGraphCommandConflict")<{
	readonly message_id: string;
}> {}

/** Reports an absent graph resource without exposing provider-native state. */
export class AgentGraphNotFound extends Data.TaggedError("AgentGraphNotFound")<{
	readonly resource: "agent_instance" | "assignment" | "orchestration_group";
	readonly id: string;
}> {}

/** Reports invalid graph topology, lifecycle intent, or visible status text. */
export class AgentGraphInvalid extends Data.TaggedError("AgentGraphInvalid")<{
	readonly message: string;
}> {}

/** Wraps an unexpected durable graph persistence failure. */
export class AgentGraphFailure extends Data.TaggedError("AgentGraphFailure")<{
	readonly cause: unknown;
}> {}

export type AgentGraphError =
	| AgentGraphCommandConflict
	| AgentGraphFailure
	| AgentGraphInvalid
	| AgentGraphNotFound;

export interface AcceptedAgentGraphCommand {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly group_id: string;
	readonly journal_sequence: number;
	readonly status: "accepted" | "duplicate";
}

export interface AgentGraphControlClaim {
	readonly action: AgentGraphControlAction;
	readonly assignment_id: string;
	readonly command_status: "completed" | "dispatching" | "failed";
	readonly group_id: string;
	readonly run_id: string;
	readonly status: "accepted" | "duplicate";
}

export interface PendingAgentRun {
	readonly agent_id: string;
	readonly assignment_id: string;
	readonly attempt: number;
	readonly engine_id: string;
	readonly expected_result: string;
	readonly group_id: string;
	readonly instructions: string;
	readonly max_concurrency: number;
	readonly permission_policy: AssignmentPermissionPolicy;
	readonly profile: string;
	readonly run_id: string;
	readonly scope: AssignmentScope;
	readonly summary_contract: string;
	readonly thread_id: string;
	readonly workspace: AssignmentWorkspace;
}

export interface AgentRunActivation {
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly run: AgentRun;
}

export interface AgentGraphRepositoryShape {
	readonly StartGroup: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly RenameAgent: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly RecordHeartbeat: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly RetryAssignment: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly ClaimControl: (
		command: CommandEnvelope,
	) => Effect.Effect<AgentGraphControlClaim, AgentGraphError>;
	readonly CompleteControl: (
		command: CommandEnvelope,
		claim: AgentGraphControlClaim,
		outcome: AgentGraphControlOutcome,
		reason?: string,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly ReadCommandEvents: (
		message_id: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly FinalizeControl: (
		claim: AgentGraphControlClaim,
		event: EventEnvelope,
	) => Effect.Effect<void, AgentGraphError>;
	readonly GetGraph: (group_id: string) => Effect.Effect<OrchestrationGraph, AgentGraphError>;
	readonly ListGroups: (
		thread_id: string,
		include_terminal: boolean,
	) => Effect.Effect<ReadonlyArray<OrchestrationGroupSummary>, AgentGraphError>;
	readonly ListGroupsSnapshot: (
		thread_id: string,
		include_terminal: boolean,
	) => Effect.Effect<OrchestrationGroupListSnapshot, AgentGraphError>;
	readonly GetPendingRuns: () => Effect.Effect<ReadonlyArray<PendingAgentRun>, AgentGraphError>;
	readonly ClaimRun: (
		run_id: string,
		instance_id: string,
	) => Effect.Effect<boolean, AgentGraphError>;
	readonly ActivateRun: (
		run_id: string,
		instance_id: string,
		native_thread_id: string,
		resume_token: unknown,
		model_id?: string,
	) => Effect.Effect<AgentRunActivation, AgentGraphError>;
	readonly FailRunStart: (
		run_id: string,
		instance_id: string,
		failure: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly RecordObservation: (
		observation: EngineObservation,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly RecordClosed: (
		run_id: string,
		state: EngineRunTerminalState,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly Recover: (
		instance_id: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
}

export function normalize_graph_error(error: unknown): AgentGraphError {
	if (
		error instanceof AgentGraphCommandConflict ||
		error instanceof AgentGraphInvalid ||
		error instanceof AgentGraphNotFound
	) {
		return error;
	}

	return new AgentGraphFailure({ cause: error });
}
