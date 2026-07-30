import { and, asc, eq, inArray } from "drizzle-orm";
import { Effect, Schema } from "effect";

import type { EventEnvelope, OrchestrationLifecycleState } from "@artisan/protocol";

import { Assignments, OrchestrationGroups, OrchestrationJoins } from "../../persistence/tables";
import { AgentGraphInvalid, AgentGraphNotFound } from "../agent-graph-model";
import {
	is_terminal_state,
	type GraphContext,
	type GraphTransaction,
	type GraphTransitionInput,
} from "./graph-context";
import type { GraphLedger } from "./graph-ledger";
import type { PersistedGraphCodecs } from "./persisted-graph-codecs";

export interface JoinEvaluation {
	readonly resolve_joins: (
		transaction: GraphTransaction,
		input: GraphTransitionInput,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
	readonly update_group_state: (
		transaction: GraphTransaction,
		input: GraphTransitionInput,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
}

/** Owns explicit join strategies, downstream release, and aggregate group state. */
export function make_join_evaluation(
	context: GraphContext,
	codecs: PersistedGraphCodecs,
	ledger: GraphLedger,
): JoinEvaluation {
	const { metadata } = context;

	const resolve_joins = (transaction: GraphTransaction, input: GraphTransitionInput) =>
		Effect.gen(function* () {
			const joins = yield* transaction
				.select()
				.from(OrchestrationJoins)
				.where(
					and(
						eq(OrchestrationJoins.group_id, input.group_id),
						eq(OrchestrationJoins.state, "joining"),
					),
				);
			const events: Array<EventEnvelope> = [];

			for (const join of joins) {
				const upstream_ids = yield* codecs.decode_json(
					Schema.NonEmptyArray(Schema.String),
					join.upstream_assignment_ids_json,
					`Join ${join.join_id} upstream assignments`,
				);
				const upstream = yield* transaction
					.select({ assignment_id: Assignments.assignment_id, state: Assignments.state })
					.from(Assignments)
					.where(inArray(Assignments.assignment_id, upstream_ids))
					.orderBy(asc(Assignments.assignment_id));

				if (upstream.length !== upstream_ids.length) {
					return yield* new AgentGraphInvalid({
						message: `Join ${join.join_id} lost an upstream assignment`,
					});
				}

				const successful = upstream.find(({ state }) => state === "complete");
				const all_terminal = upstream.every(({ state }) => is_terminal_state(state));
				const all_successful = upstream.every(({ state }) => state === "complete");
				const should_complete =
					join.strategy === "first_success" ? Boolean(successful) : all_successful;
				const should_fail =
					join.strategy === "first_success"
						? all_terminal && !successful
						: all_terminal && !all_successful;

				if (!should_complete && !should_fail) {
					continue;
				}

				const state = should_complete ? "complete" : "failed";
				const updated_at = yield* metadata.Now;
				const selected_assignment_id =
					join.strategy === "first_success" && successful
						? successful.assignment_id
						: undefined;

				yield* transaction
					.update(OrchestrationJoins)
					.set({
						selected_assignment_id: selected_assignment_id ?? null,
						state,
						updated_at,
					})
					.where(
						and(
							eq(OrchestrationJoins.join_id, join.join_id),
							eq(OrchestrationJoins.state, "joining"),
						),
					);
				events.push(
					yield* ledger.append_event(transaction, {
						agent_id: `join:${join.join_id}`,
						causation_id: input.causation_id,
						correlation_id: input.correlation_id,
						group_id: input.group_id,
						payload: {
							action: should_complete ? "resolved" : "failed",
							group_id: input.group_id,
							node_id: join.join_id,
							node_type: "join",
							state,
							type: "orchestration.graph.lifecycle",
						},
						thread_id: input.thread_id,
					}),
				);
			}

			return events;
		});

	const update_group_state = (transaction: GraphTransaction, input: GraphTransitionInput) =>
		Effect.gen(function* () {
			const [group] = yield* transaction
				.select()
				.from(OrchestrationGroups)
				.where(eq(OrchestrationGroups.group_id, input.group_id))
				.limit(1);
			const assignments = yield* transaction
				.select({ assignment_id: Assignments.assignment_id, state: Assignments.state })
				.from(Assignments)
				.where(eq(Assignments.group_id, input.group_id));
			const joins = yield* transaction
				.select({
					selected_assignment_id: OrchestrationJoins.selected_assignment_id,
					state: OrchestrationJoins.state,
					strategy: OrchestrationJoins.strategy,
					upstream_assignment_ids_json: OrchestrationJoins.upstream_assignment_ids_json,
				})
				.from(OrchestrationJoins)
				.where(eq(OrchestrationJoins.group_id, input.group_id));

			if (!group) {
				return yield* new AgentGraphNotFound({
					id: input.group_id,
					resource: "orchestration_group",
				});
			}

			const all_assignments_terminal = assignments.every(({ state }) =>
				is_terminal_state(state),
			);
			const all_joins_terminal = joins.every(({ state }) => is_terminal_state(state));
			const tolerated_assignment_ids = new Set<string>();

			for (const join of joins) {
				if (
					join.strategy !== "first_success" ||
					join.state !== "complete" ||
					!join.selected_assignment_id
				) {
					continue;
				}

				const upstream_ids = yield* codecs.decode_json(
					Schema.NonEmptyArray(Schema.String),
					join.upstream_assignment_ids_json,
					"Completed first_success join upstream assignments",
				);

				for (const assignment_id of upstream_ids) {
					tolerated_assignment_ids.add(assignment_id);
				}
			}

			const all_assignment_outcomes_accepted = assignments.every(
				({ assignment_id, state }) =>
					state === "complete" ||
					(is_terminal_state(state) && tolerated_assignment_ids.has(assignment_id)),
			);
			const has_running = assignments.some(({ state }) =>
				["queued", "running", "waiting", "blocked", "summarized"].includes(state),
			);
			const state: OrchestrationLifecycleState =
				all_assignments_terminal && all_joins_terminal
					? all_assignment_outcomes_accepted &&
						joins.every(({ state }) => state === "complete")
						? "complete"
						: assignments.every(({ state }) => state === "stopped")
							? "stopped"
							: "failed"
					: has_running
						? "running"
						: "joining";

			if (group.state === state) {
				return [];
			}

			const updated_at = yield* metadata.Now;

			yield* transaction
				.update(OrchestrationGroups)
				.set({ state, updated_at })
				.where(eq(OrchestrationGroups.group_id, input.group_id));

			return [
				yield* ledger.append_event(transaction, {
					agent_id: group.coordinator_agent_id,
					causation_id: input.causation_id,
					correlation_id: input.correlation_id,
					group_id: input.group_id,
					payload: {
						action: "aggregate_updated",
						group_id: input.group_id,
						node_id: input.group_id,
						node_type: "orchestration_group",
						state,
						type: "orchestration.graph.lifecycle",
					},
					thread_id: input.thread_id,
				}),
			];
		});

	return { resolve_joins, update_group_state };
}
