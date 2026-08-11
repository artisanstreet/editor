import { and, eq } from "drizzle-orm";
import { Effect } from "effect";

import {
	AssignmentHeartbeat,
	type AssignmentHeartbeatCommand,
	type AssignmentRetryCommand,
	type CommandEnvelope,
	type EventPayload,
} from "@artisan/protocol";

import {
	AgentInstances,
	AgentRuns,
	Assignments,
	OrchestrationGraphCommands,
	OrchestrationGroups,
} from "../../persistence/tables";
import {
	AgentGraphInvalid,
	normalize_graph_error,
	type AcceptedAgentGraphCommand,
	type AgentGraphCommand,
	type AgentGraphError,
} from "../agent-graph-model";
import {
	compact_status_text,
	graph_identity,
	is_terminal_state,
	normalize_visible_label,
	type GraphContext,
	type GraphTransaction,
} from "./graph-context";
import type { DependencyEvaluation } from "./dependency-evaluation";
import type { GraphLedger } from "./graph-ledger";
import type { GraphQuery } from "./graph-query";
import type { PersistedGraphCodecs } from "./persisted-graph-codecs";

interface ImmediateCommandResult {
	readonly agent_id: string;
	readonly assignment_id?: string;
	readonly group_id: string;
	readonly payload: EventPayload;
	readonly run_id?: string;
	readonly thread_id: string;
}

export interface AssignmentCommands {
	readonly record_heartbeat: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly rename_agent: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly retry_assignment: (
		command: CommandEnvelope,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
}

/** Owns immediate rename, heartbeat, and explicit retry command transactions. */
export function make_assignment_commands(
	context: GraphContext,
	codecs: PersistedGraphCodecs,
	dependencies: DependencyEvaluation,
	ledger: GraphLedger,
	query: GraphQuery,
): AssignmentCommands {
	const { database, metadata } = context;

	const accept_immediate = (
		command: CommandEnvelope,
		apply: (
			transaction: GraphTransaction,
			accepted_at: string,
		) => Effect.Effect<ImmediateCommandResult, unknown>,
	) =>
		Effect.gen(function* () {
			const result = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const payload = command.payload as AgentGraphCommand;
					const identity = graph_identity(payload);
					const existing = yield* ledger.read_existing_command(transaction, command);

					if (existing) {
						const events = yield* ledger.read_correlated_events(
							transaction,
							command.message_id,
						);

						return yield* ledger.command_acceptance(
							identity.group_id,
							events,
							"duplicate",
						);
					}

					const accepted_at = yield* metadata.Now;
					const applied = yield* apply(transaction, accepted_at);

					yield* ledger.insert_journal_command(transaction, command, accepted_at);

					const event = yield* ledger.append_event(transaction, {
						agent_id: applied.agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						group_id: applied.group_id,
						payload: applied.payload,
						...(applied.run_id ? { run_id: applied.run_id } : {}),
						thread_id: applied.thread_id,
					});

					yield* transaction.insert(OrchestrationGraphCommands).values({
						action: payload.type,
						assignment_id: applied.assignment_id ?? null,
						created_at: accepted_at,
						failure: null,
						group_id: applied.group_id,
						journal_sequence: event.journal_sequence,
						message_id: command.message_id,
						outcome: "accepted",
						run_id: applied.run_id ?? null,
						status: "completed",
						updated_at: accepted_at,
					});

					return {
						events: [event],
						group_id: applied.group_id,
						journal_sequence: event.journal_sequence,
						status: "accepted" as const,
					};
				}),
			);

			yield* ledger.publish_events(result.events);

			return result;
		}).pipe(Effect.mapError(normalize_graph_error));

	const rename_agent = (command: CommandEnvelope) =>
		accept_immediate(command, (transaction, accepted_at) =>
			Effect.gen(function* () {
				const payload = command.payload as AgentGraphCommand;

				if (payload.type !== "agent_instance.rename") {
					return yield* new AgentGraphInvalid({
						message: "Expected an agent rename command",
					});
				}

				const group = yield* query.read_owned_group(
					transaction,
					payload.group_id,
					command.thread_id,
				);
				const [agent] = yield* transaction
					.select()
					.from(AgentInstances)
					.where(
						and(
							eq(AgentInstances.agent_id, payload.agent_id),
							eq(AgentInstances.group_id, payload.group_id),
						),
					)
					.limit(1);

				if (!agent) {
					return yield* new AgentGraphInvalid({
						message: "The agent instance is not in this group",
					});
				}

				const names = yield* transaction
					.select({
						agent_id: AgentInstances.agent_id,
						display_name: AgentInstances.display_name,
					})
					.from(AgentInstances)
					.where(eq(AgentInstances.group_id, payload.group_id));
				const display_name = yield* normalize_visible_label(
					payload.display_name,
					"Agent display name",
				);

				if (
					names.some(
						(candidate) =>
							candidate.agent_id !== payload.agent_id &&
							candidate.display_name.toLowerCase() === display_name.toLowerCase(),
					)
				) {
					return yield* new AgentGraphInvalid({
						message: "Agent display names must be unique within a group",
					});
				}

				yield* transaction
					.update(AgentInstances)
					.set({ display_name, updated_at: accepted_at })
					.where(eq(AgentInstances.agent_id, payload.agent_id));

				return {
					agent_id: payload.agent_id,
					group_id: payload.group_id,
					payload: {
						agent_id: payload.agent_id,
						display_name,
						group_id: payload.group_id,
						type: "agent_instance.renamed",
					},
					thread_id: group.thread_id,
				};
			}),
		);

	const record_heartbeat = (command: CommandEnvelope) =>
		Effect.gen(function* () {
			const payload = command.payload as AssignmentHeartbeatCommand;
			const short_description = yield* compact_status_text(
				payload.short_description,
				"short_description",
				160,
			);
			const current_action = yield* compact_status_text(
				payload.current_action,
				"current_action",
				240,
			);
			const blocked_reason = payload.blocked_reason
				? yield* compact_status_text(payload.blocked_reason, "blocked_reason", 240)
				: undefined;

			return yield* accept_immediate(command, (transaction, accepted_at) =>
				Effect.gen(function* () {
					const { assignment, group } = yield* query.read_owned_assignment(
						transaction,
						payload.group_id,
						payload.assignment_id,
						command.thread_id,
					);

					if (is_terminal_state(assignment.state)) {
						return yield* new AgentGraphInvalid({
							message: "A terminal assignment cannot accept a heartbeat",
						});
					}

					const previous_heartbeat = assignment.heartbeat_json
						? yield* codecs.decode_json(
								AssignmentHeartbeat,
								assignment.heartbeat_json,
								`Assignment ${assignment.assignment_id} heartbeat`,
							)
						: undefined;
					const heartbeat_time = Date.parse(payload.updated_at);
					const previous_time = previous_heartbeat
						? Date.parse(previous_heartbeat.updated_at)
						: undefined;

					if (
						Number.isNaN(heartbeat_time) ||
						(previous_time !== undefined && heartbeat_time <= previous_time)
					) {
						return yield* new AgentGraphInvalid({
							message:
								"Heartbeat updated_at must advance the assignment status timestamp",
						});
					}

					const heartbeat = {
						...(blocked_reason ? { blocked_reason } : {}),
						confidence: payload.confidence,
						current_action,
						short_description,
						updated_at: payload.updated_at,
					};
					const [active_run] = assignment.active_run_id
						? yield* transaction
								.select({ state: AgentRuns.state })
								.from(AgentRuns)
								.where(eq(AgentRuns.run_id, assignment.active_run_id))
								.limit(1)
						: [];
					const state = blocked_reason
						? "blocked"
						: assignment.state === "blocked"
							? (active_run?.state ?? "running")
							: assignment.state;

					yield* transaction
						.update(Assignments)
						.set({
							heartbeat_json: JSON.stringify(heartbeat),
							state,
							updated_at: accepted_at,
						})
						.where(eq(Assignments.assignment_id, assignment.assignment_id));

					return {
						agent_id: assignment.agent_id,
						assignment_id: assignment.assignment_id,
						group_id: payload.group_id,
						payload: {
							assignment_id: assignment.assignment_id,
							group_id: payload.group_id,
							heartbeat,
							type: "assignment.heartbeat",
						},
						...(assignment.active_run_id ? { run_id: assignment.active_run_id } : {}),
						thread_id: group.thread_id,
					};
				}),
			);
		}).pipe(Effect.mapError(normalize_graph_error));

	const retry_assignment = (command: CommandEnvelope) =>
		accept_immediate(command, (transaction, accepted_at) =>
			Effect.gen(function* () {
				const payload = command.payload as AssignmentRetryCommand;
				const { assignment, group } = yield* query.read_owned_assignment(
					transaction,
					payload.group_id,
					payload.assignment_id,
					command.thread_id,
				);

				if (!is_terminal_state(assignment.state)) {
					return yield* new AgentGraphInvalid({
						message: "Only a terminal assignment can be retried",
					});
				}

				if (assignment.current_attempt >= assignment.max_attempts) {
					return yield* new AgentGraphInvalid({
						message: "The assignment has exhausted its configured attempts",
					});
				}

				yield* dependencies.assert_gates_satisfied(
					transaction,
					assignment.group_id,
					assignment.assignment_id,
				);

				const attempt = assignment.current_attempt + 1;
				const run_id = yield* metadata.MakeId("run");

				yield* transaction.insert(AgentRuns).values({
					agent_id: assignment.agent_id,
					assignment_id: assignment.assignment_id,
					attempt,
					completed_at: null,
					created_at: accepted_at,
					dispatch_status: "queued",
					execution_origin: "artisan_dispatched",
					engine_id: assignment.engine_id,
					group_id: assignment.group_id,
					last_observation_sequence: 0,
					native_resume_json: null,
					native_identity_json: null,
					native_thread_id: null,
					owner_instance_id: null,
					profile: assignment.profile,
					raw_origin_json: null,
					run_id,
					state: "queued",
					updated_at: accepted_at,
				});
				yield* transaction
					.update(Assignments)
					.set({
						active_run_id: run_id,
						current_attempt: attempt,
						state: "queued",
						updated_at: accepted_at,
					})
					.where(eq(Assignments.assignment_id, assignment.assignment_id));
				yield* transaction
					.update(OrchestrationGroups)
					.set({ state: "running", updated_at: accepted_at })
					.where(eq(OrchestrationGroups.group_id, assignment.group_id));

				return {
					agent_id: assignment.agent_id,
					assignment_id: assignment.assignment_id,
					group_id: assignment.group_id,
					payload: {
						action: "retried",
						attempt,
						group_id: assignment.group_id,
						node_id: run_id,
						node_type: "agent_run",
						state: "queued",
						type: "orchestration.graph.lifecycle",
					},
					run_id,
					thread_id: group.thread_id,
				};
			}),
		);

	return { record_heartbeat, rename_agent, retry_assignment };
}
