import { and, asc, eq, inArray } from "drizzle-orm";
import { Effect } from "effect";

import type { EventEnvelope } from "@artisan/protocol";

import {
	AgentRuns,
	Assignments,
	OrchestrationGraphEdges,
	OrchestrationJoins,
} from "../../persistence/schema";
import { AgentGraphInvalid } from "../agent-graph-model";
import {
	is_terminal_state,
	type GraphContext,
	type GraphTransaction,
	type GraphTransitionInput,
} from "./graph-context";
import type { GraphLedger } from "./graph-ledger";

export interface DependencyEvaluation {
	readonly assert_gates_satisfied: (
		transaction: GraphTransaction,
		group_id: string,
		assignment_id: string,
	) => Effect.Effect<void, unknown>;
	readonly evaluate_dependencies: (
		transaction: GraphTransaction,
		input: GraphTransitionInput,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
}

/** Owns dispatch gating and failure propagation for explicit dependencies and joins. */
export function make_dependency_evaluation(
	context: GraphContext,
	ledger: GraphLedger,
): DependencyEvaluation {
	const { metadata } = context;

	const read_gates = (transaction: GraphTransaction, group_id: string, assignment_id: string) =>
		Effect.gen(function* () {
			const dependency_edges = yield* transaction
				.select({ from_node_id: OrchestrationGraphEdges.from_node_id })
				.from(OrchestrationGraphEdges)
				.where(
					and(
						eq(OrchestrationGraphEdges.group_id, group_id),
						eq(OrchestrationGraphEdges.kind, "dependency"),
						eq(OrchestrationGraphEdges.dispatch_dependency, 1),
						eq(OrchestrationGraphEdges.to_node_id, assignment_id),
					),
				)
				.orderBy(asc(OrchestrationGraphEdges.edge_id));
			const predecessor_ids = dependency_edges.map(({ from_node_id }) => from_node_id);
			const predecessors =
				predecessor_ids.length === 0
					? []
					: yield* transaction
							.select({
								assignment_id: Assignments.assignment_id,
								state: Assignments.state,
							})
							.from(Assignments)
							.where(
								and(
									eq(Assignments.group_id, group_id),
									inArray(Assignments.assignment_id, predecessor_ids),
								),
							)
							.orderBy(asc(Assignments.assignment_id));
			const joins = yield* transaction
				.select({ join_id: OrchestrationJoins.join_id, state: OrchestrationJoins.state })
				.from(OrchestrationJoins)
				.where(
					and(
						eq(OrchestrationJoins.group_id, group_id),
						eq(OrchestrationJoins.downstream_assignment_id, assignment_id),
					),
				)
				.orderBy(asc(OrchestrationJoins.join_id));

			if (predecessors.length !== predecessor_ids.length) {
				return yield* new AgentGraphInvalid({
					message: `Assignment ${assignment_id} lost a dependency predecessor`,
				});
			}

			return { joins, predecessors };
		});

	const assert_gates_satisfied = (
		transaction: GraphTransaction,
		group_id: string,
		assignment_id: string,
	) =>
		Effect.gen(function* () {
			const gates = yield* read_gates(transaction, group_id, assignment_id);
			const dependencies_complete = gates.predecessors.every(
				({ state }) => state === "complete",
			);
			const joins_complete = gates.joins.every(({ state }) => state === "complete");

			if (!dependencies_complete || !joins_complete) {
				return yield* new AgentGraphInvalid({
					message: `Assignment ${assignment_id} cannot run until all dependency gates complete`,
				});
			}
		});

	const evaluate_dependencies = (transaction: GraphTransaction, input: GraphTransitionInput) =>
		Effect.gen(function* () {
			const gated_assignments = yield* transaction
				.select()
				.from(Assignments)
				.where(
					and(
						eq(Assignments.group_id, input.group_id),
						inArray(Assignments.state, ["blocked", "joining"]),
					),
				)
				.orderBy(asc(Assignments.created_at), asc(Assignments.assignment_id));
			const events: Array<EventEnvelope> = [];

			for (const assignment of gated_assignments) {
				if (!assignment.active_run_id) {
					continue;
				}

				const gates = yield* read_gates(
					transaction,
					assignment.group_id,
					assignment.assignment_id,
				);
				const failed_predecessor = gates.predecessors.find(
					({ state }) => is_terminal_state(state) && state !== "complete",
				);
				const failed_join = gates.joins.find(({ state }) => state === "failed");
				const should_fail = Boolean(failed_predecessor || failed_join);
				const should_release =
					gates.predecessors.every(({ state }) => state === "complete") &&
					gates.joins.every(({ state }) => state === "complete");

				if (!should_fail && !should_release) {
					continue;
				}

				const updated_at = yield* metadata.Now;
				const state = should_fail ? "failed" : "queued";
				const assignment_updated = yield* transaction
					.update(Assignments)
					.set({ state, updated_at })
					.where(
						and(
							eq(Assignments.assignment_id, assignment.assignment_id),
							eq(Assignments.active_run_id, assignment.active_run_id),
							inArray(Assignments.state, ["blocked", "joining"]),
						),
					)
					.returning({ assignment_id: Assignments.assignment_id });

				if (assignment_updated.length !== 1) {
					continue;
				}

				const run_updated = yield* transaction
					.update(AgentRuns)
					.set({
						...(should_fail ? { completed_at: updated_at } : {}),
						dispatch_status: should_fail ? "terminal" : "queued",
						state,
						updated_at,
					})
					.where(
						and(
							eq(AgentRuns.run_id, assignment.active_run_id),
							eq(AgentRuns.dispatch_status, "blocked"),
						),
					)
					.returning({ run_id: AgentRuns.run_id });

				if (run_updated.length !== 1) {
					return yield* new AgentGraphInvalid({
						message: `Assignment ${assignment.assignment_id} lost its gated run`,
					});
				}

				const action = should_fail
					? failed_join
						? "join_failed"
						: "dependency_failed"
					: gates.joins.length > 0 && gates.predecessors.length > 0
						? "gates_released"
						: gates.joins.length > 0
							? "join_released"
							: "dependencies_released";

				events.push(
					yield* ledger.append_event(transaction, {
						agent_id: assignment.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						group_id: input.group_id,
						payload: {
							action,
							attempt: assignment.current_attempt,
							group_id: input.group_id,
							node_id: assignment.assignment_id,
							node_type: "assignment",
							state,
							type: "orchestration.graph.lifecycle",
						},
						run_id: assignment.active_run_id,
						thread_id: input.thread_id,
					}),
					yield* ledger.append_event(transaction, {
						agent_id: assignment.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						group_id: input.group_id,
						payload: {
							action,
							attempt: assignment.current_attempt,
							group_id: input.group_id,
							node_id: assignment.active_run_id,
							node_type: "agent_run",
							state,
							type: "orchestration.graph.lifecycle",
						},
						run_id: assignment.active_run_id,
						thread_id: input.thread_id,
					}),
				);
			}

			return events;
		});

	return { assert_gates_satisfied, evaluate_dependencies };
}
