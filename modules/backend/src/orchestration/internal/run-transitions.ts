import { and, eq, inArray } from "drizzle-orm";
import { Effect } from "effect";

import type { EventEnvelope, RawOrigin } from "@artisan/protocol";

import { AgentRuns, Assignments } from "../../persistence/tables";
import type { GraphContext, GraphTransaction } from "./graph-context";
import type { GraphAdvancement } from "./graph-advancement";
import type { GraphLedger } from "./graph-ledger";

interface TerminalRunInput {
	readonly causation_id: string;
	readonly correlation_id: string;
	readonly raw_origin?: RawOrigin;
	readonly run: typeof AgentRuns.$inferSelect;
	readonly state: "complete" | "failed" | "stopped";
	readonly thread_id: string;
}

export interface RunTransitions {
	readonly transition_terminal_run: (
		transaction: GraphTransaction,
		input: TerminalRunInput,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
}

/** Owns monotonic attempt termination, retries, and downstream join advancement. */
export function make_run_transitions(
	context: GraphContext,
	advancement: GraphAdvancement,
	ledger: GraphLedger,
): RunTransitions {
	const { metadata } = context;

	const queue_retry = (
		transaction: GraphTransaction,
		run: typeof AgentRuns.$inferSelect,
		updated_at: string,
	) =>
		Effect.gen(function* () {
			const [assignment] = yield* transaction
				.select()
				.from(Assignments)
				.where(
					and(
						eq(Assignments.assignment_id, run.assignment_id),
						eq(Assignments.active_run_id, run.run_id),
					),
				)
				.limit(1);

			if (!assignment || run.attempt >= assignment.max_attempts) {
				return undefined;
			}

			const attempt = run.attempt + 1;
			const retry_run_id = yield* metadata.MakeId("run");

			yield* transaction.insert(AgentRuns).values({
				agent_id: run.agent_id,
				assignment_id: run.assignment_id,
				attempt,
				completed_at: null,
				created_at: updated_at,
				dispatch_status: "queued",
				engine_id: run.engine_id,
				group_id: run.group_id,
				last_observation_sequence: 0,
				native_resume_json: null,
				native_identity_json: null,
				native_thread_id: null,
				owner_instance_id: null,
				profile: run.profile,
				raw_origin_json: null,
				run_id: retry_run_id,
				state: "queued",
				updated_at,
			});
			yield* transaction
				.update(Assignments)
				.set({
					active_run_id: retry_run_id,
					current_attempt: attempt,
					state: "queued",
					updated_at,
				})
				.where(
					and(
						eq(Assignments.assignment_id, run.assignment_id),
						eq(Assignments.active_run_id, run.run_id),
					),
				);

			return { attempt, retry_run_id };
		});

	const transition_terminal_run = (transaction: GraphTransaction, input: TerminalRunInput) =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;
			const updated = yield* transaction
				.update(AgentRuns)
				.set({
					completed_at: updated_at,
					dispatch_status: "terminal",
					state: input.state,
					updated_at,
				})
				.where(
					and(
						eq(AgentRuns.run_id, input.run.run_id),
						inArray(AgentRuns.dispatch_status, ["active", "dispatching"]),
					),
				)
				.returning({ run_id: AgentRuns.run_id });

			if (updated.length === 0) {
				return [];
			}

			const assignment_updated = yield* transaction
				.update(Assignments)
				.set({ state: input.state, updated_at })
				.where(
					and(
						eq(Assignments.assignment_id, input.run.assignment_id),
						eq(Assignments.active_run_id, input.run.run_id),
					),
				)
				.returning({ assignment_id: Assignments.assignment_id });
			const events: Array<EventEnvelope> = [
				yield* ledger.append_event(transaction, {
					agent_id: input.run.agent_id,
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					group_id: input.run.group_id,
					payload: {
						action: "finished",
						attempt: input.run.attempt,
						group_id: input.run.group_id,
						node_id: input.run.run_id,
						node_type: "agent_run",
						state: input.state,
						type: "orchestration.graph.lifecycle",
					},
					...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
					run_id: input.run.run_id,
					thread_id: input.thread_id,
				}),
			];

			if (assignment_updated.length === 1) {
				events.push(
					yield* ledger.append_event(transaction, {
						agent_id: input.run.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						group_id: input.run.group_id,
						payload: {
							action: "attempt_finished",
							attempt: input.run.attempt,
							group_id: input.run.group_id,
							node_id: input.run.assignment_id,
							node_type: "assignment",
							state: input.state,
							type: "orchestration.graph.lifecycle",
						},
						...(input.raw_origin ? { raw_origin: input.raw_origin } : {}),
						run_id: input.run.run_id,
						thread_id: input.thread_id,
					}),
				);
			}

			const retry =
				input.state === "failed" && assignment_updated.length === 1
					? yield* queue_retry(transaction, input.run, updated_at)
					: undefined;

			if (retry) {
				events.push(
					yield* ledger.append_event(transaction, {
						agent_id: input.run.agent_id,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						group_id: input.run.group_id,
						payload: {
							action: "retry_queued",
							attempt: retry.attempt,
							group_id: input.run.group_id,
							node_id: retry.retry_run_id,
							node_type: "agent_run",
							state: "queued",
							type: "orchestration.graph.lifecycle",
						},
						thread_id: input.thread_id,
					}),
				);
			}

			events.push(
				...(yield* advancement.advance_graph(transaction, {
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					group_id: input.run.group_id,
					thread_id: input.thread_id,
				})),
			);

			return events;
		});

	return { transition_terminal_run };
}
