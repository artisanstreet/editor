import { eq } from "drizzle-orm";
import { Effect } from "effect";

import type { CommandEnvelope, EventEnvelope } from "@artisan/protocol";

import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationGraphCommands,
	OrchestrationGraphEdges,
	OrchestrationGroups,
	OrchestrationJoins,
	Threads,
} from "../../persistence/tables";
import {
	AgentGraphInvalid,
	AgentGraphNotFound,
	normalize_graph_error,
	type AcceptedAgentGraphCommand,
	type AgentGraphCommand,
	type AgentGraphError,
} from "../agent-graph-model";
import type { GraphContext } from "./graph-context";
import type { GraphLedger } from "./graph-ledger";
import type { GraphTopology } from "./graph-topology";

export interface GroupStart {
	readonly start_group: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
}

/** Owns atomic orchestration_group creation and initial topology persistence. */
export function make_group_start(
	context: GraphContext,
	ledger: GraphLedger,
	topology: GraphTopology,
): GroupStart {
	const { database, metadata } = context;

	const start_group = (command: CommandEnvelope) =>
		Effect.gen(function* () {
			const payload = command.payload as AgentGraphCommand;

			if (payload.type !== "orchestration.group.start") {
				return yield* new AgentGraphInvalid({ message: "Expected a group start command" });
			}

			yield* topology.validate_topology(payload);

			const result = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const existing = yield* ledger.read_existing_command(transaction, command);

					if (existing) {
						const events = yield* ledger.read_correlated_events(
							transaction,
							command.message_id,
						);

						return yield* ledger.command_acceptance(
							payload.group_id,
							events,
							"duplicate",
						);
					}

					const [thread] = yield* transaction
						.select({ thread_id: Threads.thread_id })
						.from(Threads)
						.where(eq(Threads.thread_id, command.thread_id))
						.limit(1);

					if (!thread) {
						return yield* new AgentGraphNotFound({
							id: command.thread_id,
							resource: "orchestration_group",
						});
					}

					const [existing_group] = yield* transaction
						.select({ group_id: OrchestrationGroups.group_id })
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, payload.group_id))
						.limit(1);

					if (existing_group) {
						return yield* new AgentGraphInvalid({
							message: `Orchestration group ${payload.group_id} already exists`,
						});
					}

					const created_at = yield* metadata.Now;
					const coordinator_agent_id =
						command.agent_id ?? (yield* metadata.MakeId("agent"));
					const name_bank =
						payload.name_bank ?? (yield* context.agent_name_catalog.Names);
					const existing_agents = yield* transaction
						.select({ display_name: AgentInstances.display_name })
						.from(AgentInstances)
						.innerJoin(
							OrchestrationGroups,
							eq(AgentInstances.group_id, OrchestrationGroups.group_id),
						)
						.where(eq(OrchestrationGroups.thread_id, command.thread_id));
					const allocated = yield* topology.allocate_agent_instances(
						payload.assignments,
						payload.group_id,
						coordinator_agent_id,
						name_bank,
						existing_agents.map((agent) => agent.display_name),
						created_at,
					);
					const join_blocked_assignment_ids = new Set(
						(payload.joins ?? [])
							.map(({ downstream_assignment_id }) => downstream_assignment_id)
							.filter((value): value is string => Boolean(value)),
					);
					const dependency_blocked_assignment_ids = new Set(
						(payload.edges ?? [])
							.filter(({ kind }) => kind === "dependency")
							.map(({ to_node_id }) => to_node_id),
					);

					yield* transaction.insert(OrchestrationGroups).values({
						coordinator_agent_id,
						created_at,
						group_id: payload.group_id,
						journal_sequence: 0,
						max_concurrency: payload.max_concurrency ?? 4,
						state: "queued",
						thread_id: command.thread_id,
						updated_at: created_at,
						version: 0,
					});
					yield* transaction.insert(AgentInstances).values([...allocated.instances]);

					for (const assignment of payload.assignments) {
						const run_id = yield* metadata.MakeId("run");
						const waiting_for_join = join_blocked_assignment_ids.has(
							assignment.assignment_id,
						);
						const waiting_for_dependency = dependency_blocked_assignment_ids.has(
							assignment.assignment_id,
						);
						const blocked = waiting_for_join || waiting_for_dependency;
						const initial_state = waiting_for_join
							? "joining"
							: waiting_for_dependency
								? "blocked"
								: "queued";
						const agent_id = allocated.assignment_agents.get(assignment.assignment_id);
						const role = allocated.assignment_roles.get(assignment.assignment_id);
						if (agent_id === undefined || role === undefined)
							return yield* new AgentGraphInvalid({
								message: `Assignment ${assignment.assignment_id} has no allocated agent identity`,
							});

						yield* transaction.insert(Assignments).values({
							active_run_id: run_id,
							agent_id,
							assignment_id: assignment.assignment_id,
							created_at,
							current_attempt: 1,
							engine_id: assignment.engine_id,
							expected_result: assignment.expected_result,
							group_id: payload.group_id,
							heartbeat_json: null,
							instructions: assignment.instructions,
							max_attempts: assignment.max_attempts ?? 1,
							parent_node_id: assignment.parent_node_id,
							permission_policy_json: JSON.stringify(assignment.permission_policy),
							profile: assignment.profile,
							role,
							scope_json: JSON.stringify(assignment.scope),
							state: initial_state,
							summary_contract: assignment.summary_contract,
							updated_at: created_at,
							workspace_json: JSON.stringify(assignment.workspace),
						});
						yield* transaction.insert(AgentRuns).values({
							agent_id,
							assignment_id: assignment.assignment_id,
							attempt: 1,
							completed_at: null,
							created_at,
							dispatch_status: blocked ? "blocked" : "queued",
							engine_id: assignment.engine_id,
							execution_origin: "artisan_dispatched",
							group_id: payload.group_id,
							last_observation_sequence: 0,
							native_resume_json: null,
							native_identity_json: null,
							native_thread_id: null,
							owner_instance_id: null,
							profile: assignment.profile,
							raw_origin_json: null,
							run_id,
							state: initial_state,
							updated_at: created_at,
						});
					}

					if (payload.joins?.length) {
						yield* transaction.insert(OrchestrationJoins).values(
							payload.joins.map((join) => ({
								created_at,
								downstream_assignment_id: join.downstream_assignment_id ?? null,
								group_id: payload.group_id,
								join_id: join.join_id,
								selected_assignment_id: null,
								state: "joining",
								strategy: join.strategy,
								updated_at: created_at,
								upstream_assignment_ids_json: JSON.stringify(
									join.upstream_assignment_ids,
								),
							})),
						);
					}

					const structural_edges = [
						...payload.assignments.map((assignment) => ({
							edge_id: `edge:${payload.group_id}:parent:${assignment.assignment_id}`,
							from_node_id: assignment.parent_node_id,
							group_id: payload.group_id,
							kind: "dependency" as const,
							dispatch_dependency: 0,
							to_node_id: assignment.assignment_id,
						})),
						...(payload.joins ?? []).flatMap((join) => [
							...join.upstream_assignment_ids.map((assignment_id) => ({
								edge_id: `edge:${payload.group_id}:join:${join.join_id}:${assignment_id}`,
								from_node_id: assignment_id,
								group_id: payload.group_id,
								kind: "result" as const,
								dispatch_dependency: 0,
								to_node_id: join.join_id,
							})),
							...(join.downstream_assignment_id
								? [
										{
											edge_id: `edge:${payload.group_id}:join:${join.join_id}:downstream`,
											from_node_id: join.join_id,
											group_id: payload.group_id,
											kind: "dependency" as const,
											dispatch_dependency: 0,
											to_node_id: join.downstream_assignment_id,
										},
									]
								: []),
						]),
					];
					const all_edges = [
						...structural_edges,
						...(payload.edges ?? []).map((edge) => ({
							...edge,
							dispatch_dependency: edge.kind === "dependency" ? 1 : 0,
							group_id: payload.group_id,
						})),
					];

					if (
						new Set(all_edges.map(({ edge_id }) => edge_id)).size !== all_edges.length
					) {
						return yield* new AgentGraphInvalid({
							message: "Explicit graph edges conflict with structural graph edges",
						});
					}

					yield* transaction.insert(OrchestrationGraphEdges).values(all_edges);
					yield* ledger.insert_journal_command(transaction, command, created_at);

					const events: Array<EventEnvelope> = [
						yield* ledger.append_event(transaction, {
							agent_id: coordinator_agent_id,
							causation_id: command.message_id,
							correlation_id: command.message_id,
							group_id: payload.group_id,
							payload: {
								action: "started",
								group_id: payload.group_id,
								node_id: payload.group_id,
								node_type: "orchestration_group",
								state: "queued",
								type: "orchestration.graph.lifecycle",
							},
							thread_id: command.thread_id,
						}),
					];

					for (const assignment of payload.assignments) {
						const agent_id = allocated.assignment_agents.get(assignment.assignment_id);
						if (agent_id === undefined)
							return yield* new AgentGraphInvalid({
								message: `Assignment ${assignment.assignment_id} has no allocated agent identity`,
							});
						const waiting_for_join = join_blocked_assignment_ids.has(
							assignment.assignment_id,
						);
						const waiting_for_dependency = dependency_blocked_assignment_ids.has(
							assignment.assignment_id,
						);
						const state = waiting_for_join
							? "joining"
							: waiting_for_dependency
								? "blocked"
								: "queued";
						const action = waiting_for_join
							? "waiting_for_join"
							: waiting_for_dependency
								? "waiting_for_dependencies"
								: "queued";

						events.push(
							yield* ledger.append_event(transaction, {
								agent_id,
								causation_id: command.message_id,
								correlation_id: command.message_id,
								group_id: payload.group_id,
								payload: {
									action,
									attempt: 1,
									group_id: payload.group_id,
									node_id: assignment.assignment_id,
									node_type: "assignment",
									state,
									type: "orchestration.graph.lifecycle",
								},
								thread_id: command.thread_id,
							}),
						);
					}

					const latest_event = events.at(-1);
					if (latest_event === undefined)
						return yield* new AgentGraphInvalid({
							message: `Group ${payload.group_id} produced no lifecycle events`,
						});
					const journal_sequence = latest_event.journal_sequence;

					yield* transaction.insert(OrchestrationGraphCommands).values({
						action: payload.type,
						assignment_id: null,
						created_at,
						failure: null,
						group_id: payload.group_id,
						journal_sequence,
						message_id: command.message_id,
						outcome: "accepted",
						run_id: null,
						status: "completed",
						updated_at: created_at,
					});

					return {
						events,
						group_id: payload.group_id,
						journal_sequence,
						status: "accepted" as const,
					};
				}),
			);

			yield* ledger.publish_events(result.events);

			return result;
		}).pipe(Effect.mapError(normalize_graph_error));

	return { start_group };
}
