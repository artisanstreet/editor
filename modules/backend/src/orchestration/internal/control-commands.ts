import { and, eq } from "drizzle-orm";
import { Effect } from "effect";

import type { CommandEnvelope, EventEnvelope } from "@artisan/protocol";

import { AgentRuns, OrchestrationGraphCommands } from "../../persistence/schema";
import {
	AgentGraphInvalid,
	normalize_graph_error,
	type AcceptedAgentGraphCommand,
	type AgentGraphCommand,
	type AgentGraphControlClaim,
	type AgentGraphControlOutcome,
	type AgentGraphError,
} from "../agent-graph-model";
import { control_action, is_terminal_state, type GraphContext } from "./graph-context";
import type { GraphLedger } from "./graph-ledger";
import type { GraphQuery } from "./graph-query";

type ControlPayload = Extract<
	AgentGraphCommand,
	{
		readonly type:
			| "assignment.pause"
			| "assignment.resume"
			| "assignment.steer"
			| "assignment.stop";
	}
>;

export interface ControlCommands {
	readonly claim_control: (
		command: CommandEnvelope,
	) => Effect.Effect<AgentGraphControlClaim, AgentGraphError>;
	readonly complete_control: (
		command: CommandEnvelope,
		claim: AgentGraphControlClaim,
		outcome: AgentGraphControlOutcome,
		reason?: string,
	) => Effect.Effect<AcceptedAgentGraphCommand, AgentGraphError>;
	readonly finalize_control: (
		claim: AgentGraphControlClaim,
		event: EventEnvelope,
	) => Effect.Effect<void, AgentGraphError>;
	readonly read_command_events: (
		message_id: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
}

/** Owns claim-before-side-effect idempotency for external assignment controls. */
export function make_control_commands(
	context: GraphContext,
	ledger: GraphLedger,
	query: GraphQuery,
): ControlCommands {
	const { database, metadata } = context;

	const claim_control = (command: CommandEnvelope) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const graph_payload = command.payload as AgentGraphCommand;
					const action = control_action(graph_payload);

					if (!action) {
						return yield* new AgentGraphInvalid({
							message: "Expected an assignment control command",
						});
					}

					const payload = graph_payload as ControlPayload;
					const existing = yield* ledger.read_existing_command(transaction, command);

					if (existing) {
						if (
							existing.group_id !== payload.group_id ||
							existing.assignment_id !== payload.assignment_id ||
							existing.action !== action ||
							!existing.run_id
						) {
							return yield* new AgentGraphInvalid({
								message: "The graph control claim does not match its command",
							});
						}

						return {
							action,
							assignment_id: payload.assignment_id,
							command_status:
								existing.status as AgentGraphControlClaim["command_status"],
							group_id: payload.group_id,
							run_id: existing.run_id,
							status: "duplicate" as const,
						};
					}

					const { assignment } = yield* query.read_owned_assignment(
						transaction,
						payload.group_id,
						payload.assignment_id,
						command.thread_id,
					);

					if (!assignment.active_run_id || is_terminal_state(assignment.state)) {
						return yield* new AgentGraphInvalid({
							message: "The assignment has no active run attempt",
						});
					}

					const [run] = yield* transaction
						.select()
						.from(AgentRuns)
						.where(eq(AgentRuns.run_id, assignment.active_run_id))
						.limit(1);

					if (!run || run.dispatch_status !== "active" || is_terminal_state(run.state)) {
						return yield* new AgentGraphInvalid({
							message: "The assignment run is not available for control",
						});
					}

					const accepted_at = yield* metadata.Now;

					yield* ledger.insert_journal_command(transaction, command, accepted_at);
					yield* transaction.insert(OrchestrationGraphCommands).values({
						action,
						assignment_id: assignment.assignment_id,
						created_at: accepted_at,
						failure: null,
						group_id: assignment.group_id,
						journal_sequence: null,
						message_id: command.message_id,
						outcome: null,
						run_id: run.run_id,
						status: "dispatching",
						updated_at: accepted_at,
					});

					return {
						action,
						assignment_id: assignment.assignment_id,
						command_status: "dispatching" as const,
						group_id: assignment.group_id,
						run_id: run.run_id,
						status: "accepted" as const,
					};
				}),
			)
			.pipe(Effect.mapError(normalize_graph_error));

	const complete_control = (
		command: CommandEnvelope,
		claim: AgentGraphControlClaim,
		outcome: AgentGraphControlOutcome,
		reason?: string,
	) =>
		Effect.gen(function* () {
			const result = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [stored_claim] = yield* transaction
						.select()
						.from(OrchestrationGraphCommands)
						.where(
							and(
								eq(OrchestrationGraphCommands.message_id, command.message_id),
								eq(OrchestrationGraphCommands.group_id, claim.group_id),
								eq(OrchestrationGraphCommands.assignment_id, claim.assignment_id),
								eq(OrchestrationGraphCommands.run_id, claim.run_id),
								eq(OrchestrationGraphCommands.status, "dispatching"),
							),
						)
						.limit(1);

					if (!stored_claim || stored_claim.action !== claim.action) {
						return yield* new AgentGraphInvalid({
							message: "Graph control completion lost its exact dispatching claim",
						});
					}

					const { assignment, group } = yield* query.read_owned_assignment(
						transaction,
						claim.group_id,
						claim.assignment_id,
						command.thread_id,
					);
					const event = yield* ledger.append_event(transaction, {
						agent_id: assignment.agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						group_id: claim.group_id,
						payload: {
							action: claim.action,
							assignment_id: claim.assignment_id,
							group_id: claim.group_id,
							outcome,
							...(reason ? { reason } : {}),
							type: "assignment.control",
						},
						run_id: claim.run_id,
						thread_id: group.thread_id,
					});
					const updated_at = yield* metadata.Now;
					const updated = yield* transaction
						.update(OrchestrationGraphCommands)
						.set({
							failure: outcome === "accepted" ? null : (reason ?? outcome),
							journal_sequence: event.journal_sequence,
							outcome,
							status:
								outcome === "accepted" || outcome === "unsupported"
									? "completed"
									: "failed",
							updated_at,
						})
						.where(
							and(
								eq(OrchestrationGraphCommands.message_id, command.message_id),
								eq(OrchestrationGraphCommands.run_id, claim.run_id),
								eq(OrchestrationGraphCommands.status, "dispatching"),
							),
						)
						.returning({ message_id: OrchestrationGraphCommands.message_id });

					if (updated.length !== 1) {
						return yield* new AgentGraphInvalid({
							message: "Graph control completion did not update its exact claim",
						});
					}

					return {
						events: [event],
						group_id: claim.group_id,
						journal_sequence: event.journal_sequence,
						status: claim.status,
					};
				}),
			);

			yield* ledger.publish_events(result.events);

			return result;
		}).pipe(Effect.mapError(normalize_graph_error));

	const read_command_events = (message_id: string) =>
		ledger
			.read_correlated_events(database.client, message_id)
			.pipe(Effect.mapError(normalize_graph_error));

	const finalize_control = (claim: AgentGraphControlClaim, event: EventEnvelope) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					if (
						event.payload.type !== "assignment.control" ||
						event.payload.group_id !== claim.group_id ||
						event.payload.assignment_id !== claim.assignment_id ||
						event.payload.action !== claim.action
					) {
						return yield* new AgentGraphInvalid({
							message: "Correlated graph control event does not match its claim",
						});
					}

					const updated_at = yield* metadata.Now;
					const updated = yield* transaction
						.update(OrchestrationGraphCommands)
						.set({
							failure:
								event.payload.outcome === "accepted"
									? null
									: (event.payload.reason ?? event.payload.outcome),
							journal_sequence: event.journal_sequence,
							outcome: event.payload.outcome,
							status:
								event.payload.outcome === "accepted" ||
								event.payload.outcome === "unsupported"
									? "completed"
									: "failed",
							updated_at,
						})
						.where(
							and(
								eq(OrchestrationGraphCommands.message_id, event.correlation_id),
								eq(OrchestrationGraphCommands.group_id, claim.group_id),
								eq(OrchestrationGraphCommands.assignment_id, claim.assignment_id),
								eq(OrchestrationGraphCommands.run_id, claim.run_id),
								eq(OrchestrationGraphCommands.status, "dispatching"),
							),
						)
						.returning({ message_id: OrchestrationGraphCommands.message_id });

					if (updated.length !== 1) {
						return yield* new AgentGraphInvalid({
							message: "Graph control replay did not finalize its exact claim",
						});
					}
				}),
			)
			.pipe(Effect.mapError(normalize_graph_error));

	return { claim_control, complete_control, finalize_control, read_command_events };
}
