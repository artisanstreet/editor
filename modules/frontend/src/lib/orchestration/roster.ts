import type {
	AgentInstance,
	Assignment,
	OrchestrationGraph,
	OrchestrationLifecycleState,
} from "@artisan/protocol";

export interface OrchestrationRosterEntry {
	readonly agent_id: string;
	readonly display_name: string;
	readonly engine_id: string;
	readonly group_id: string;
	/** The coordinator's requested model profile; thread policy may override it. */
	readonly profile: string;
	readonly run_id: string | undefined;
	readonly state: OrchestrationLifecycleState;
	readonly status: string;
}

export type ThreadOrchestrationState =
	| { readonly _tag: "None" }
	| {
			readonly _tag: "Ready";
			readonly entries: ReadonlyArray<OrchestrationRosterEntry>;
			readonly thread_id: string;
	  };

const terminal_states = new Set<OrchestrationLifecycleState>([
	"summarized",
	"stopped",
	"failed",
	"complete",
]);

export const IsActiveOrchestrationState = (state: OrchestrationLifecycleState) =>
	!terminal_states.has(state);

const status_for_state: Readonly<Record<OrchestrationLifecycleState, string>> = {
	blocked: "Blocked on the next step",
	complete: "Completed",
	failed: "Failed",
	joining: "Joining results",
	queued: "Queued to begin",
	running: "Working",
	stopped: "Stopped",
	summarized: "Summarized",
	waiting: "Waiting for results",
};

const display_name_for = (agent_instances: ReadonlyArray<AgentInstance>, agent_id: string) =>
	agent_instances.find((agent) => agent.agent_id === agent_id)?.display_name;

const status_for_assignment = (assignment: Assignment) =>
	assignment.heartbeat?.current_action ??
	assignment.heartbeat?.short_description ??
	status_for_state[assignment.state];

/**
 * Projects only Artisan-owned identities and observable orchestration state.
 * Provider thread IDs and raw origins deliberately never cross this boundary.
 */
export const RosterForOrchestrationGraphs = (
	graphs: ReadonlyArray<OrchestrationGraph>,
): ReadonlyArray<OrchestrationRosterEntry> =>
	graphs.flatMap((graph) => {
		if (!IsActiveOrchestrationState(graph.group.state)) return [];
		const workers = graph.assignments.flatMap((assignment) => {
			if (assignment.agent_id === graph.group.coordinator_agent_id) return [];
			if (!IsActiveOrchestrationState(assignment.state)) return [];
			const display_name = display_name_for(graph.agent_instances, assignment.agent_id);
			if (display_name === undefined) return [];
			const run =
				assignment.active_run_id === undefined
					? undefined
					: graph.agent_runs.find(
							(candidate) => candidate.run_id === assignment.active_run_id,
						);
			return [
				{
					agent_id: assignment.agent_id,
					display_name,
					engine_id: assignment.engine_id,
					group_id: graph.group.group_id,
					profile: assignment.profile,
					run_id: run?.run_id ?? assignment.active_run_id,
					state: assignment.state,
					status: status_for_assignment(assignment),
				},
			];
		});

		return workers;
	});
