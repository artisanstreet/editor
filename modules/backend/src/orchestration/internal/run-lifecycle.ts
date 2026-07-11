import { and, asc, eq, inArray, ne, notExists } from "drizzle-orm";
import { Effect, Schema } from "effect";

import type { EngineObservation, EngineRunTerminalState } from "@artisan/engines";
import {
	AgentRun,
	type EventEnvelope,
	type ProviderNativeIdentity,
	type RawOrigin,
} from "@artisan/protocol";

import {
	AgentRuns,
	Assignments,
	OrchestrationArtifacts,
	OrchestrationGroups,
	ThreadErasureClaims,
} from "../../persistence/schema";
import {
	AgentGraphInvalid,
	AgentGraphNotFound,
	normalize_graph_error,
	type AgentGraphError,
	type AgentRunActivation,
} from "../agent-graph-model";
import { is_terminal_state, terminal_state_from_engine, type GraphContext } from "./graph-context";
import type { GraphLedger } from "./graph-ledger";
import type { RawObservationLedger } from "./raw-observation-ledger";
import type { RunTransitions } from "./run-transitions";

export interface RunLifecycle {
	readonly activate_run: (
		run_id: string,
		instance_id: string,
		native_thread_id: string,
		resume_token: unknown,
	) => Effect.Effect<AgentRunActivation, AgentGraphError>;
	readonly claim_run: (
		run_id: string,
		instance_id: string,
	) => Effect.Effect<boolean, AgentGraphError>;
	readonly fail_run_start: (
		run_id: string,
		instance_id: string,
		failure: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly record_closed: (
		run_id: string,
		state: EngineRunTerminalState,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly record_observation: (
		observation: EngineObservation,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
	readonly recover: (
		instance_id: string,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, AgentGraphError>;
}

/** Owns provider observation sequencing, run activation, and restart recovery. */
export function make_run_lifecycle(
	context: GraphContext,
	ledger: GraphLedger,
	raw_observations: RawObservationLedger,
	transitions: RunTransitions,
): RunLifecycle {
	const { database, metadata } = context;

	const claim_run = (run_id: string, instance_id: string) =>
		database.client
			.update(AgentRuns)
			.set({ dispatch_status: "dispatching", owner_instance_id: instance_id })
			.where(
				and(
					eq(AgentRuns.run_id, run_id),
					eq(AgentRuns.dispatch_status, "queued"),
					notExists(
						database.client
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(OrchestrationGroups)
							.innerJoin(
								ThreadErasureClaims,
								eq(ThreadErasureClaims.thread_id, OrchestrationGroups.thread_id),
							)
							.where(eq(OrchestrationGroups.group_id, AgentRuns.group_id)),
					),
				),
			)
			.returning({ run_id: AgentRuns.run_id })
			.pipe(
				Effect.map((rows) => rows.length === 1),
				Effect.mapError(normalize_graph_error),
			);

	const activate_run = (
		run_id: string,
		instance_id: string,
		native_thread_id: string,
		resume_token: unknown,
	) =>
		Effect.gen(function* () {
			const result = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [run] = yield* transaction
						.select()
						.from(AgentRuns)
						.where(
							and(
								eq(AgentRuns.run_id, run_id),
								eq(AgentRuns.owner_instance_id, instance_id),
								eq(AgentRuns.dispatch_status, "dispatching"),
							),
						)
						.limit(1);

					if (!run) {
						return yield* new AgentGraphInvalid({
							message: `Agent run ${run_id} lost its dispatch claim`,
						});
					}

					const [group] = yield* transaction
						.select()
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, run.group_id))
						.limit(1);

					if (!group) {
						return yield* new AgentGraphNotFound({
							id: run.group_id,
							resource: "orchestration_group",
						});
					}

					const updated_at = yield* metadata.Now;
					const raw_origin = {
						provider: run.engine_id,
						reference: native_thread_id,
					} satisfies RawOrigin;
					const native_identity = {
						thread_id: native_thread_id,
					} satisfies ProviderNativeIdentity;
					const [updated_run] = yield* transaction
						.update(AgentRuns)
						.set({
							dispatch_status: "active",
							native_identity_json: JSON.stringify(native_identity),
							native_resume_json: JSON.stringify(resume_token),
							native_thread_id,
							raw_origin_json: JSON.stringify(raw_origin),
							state: "running",
							updated_at,
						})
						.where(
							and(
								eq(AgentRuns.run_id, run_id),
								eq(AgentRuns.owner_instance_id, instance_id),
								eq(AgentRuns.dispatch_status, "dispatching"),
							),
						)
						.returning();

					if (!updated_run) {
						return yield* new AgentGraphInvalid({
							message: `Agent run ${run_id} activation did not update its claim`,
						});
					}

					yield* transaction
						.update(Assignments)
						.set({ state: "running", updated_at })
						.where(
							and(
								eq(Assignments.assignment_id, run.assignment_id),
								eq(Assignments.active_run_id, run.run_id),
							),
						);
					yield* transaction
						.update(OrchestrationGroups)
						.set({ state: "running", updated_at })
						.where(eq(OrchestrationGroups.group_id, run.group_id));

					const event = yield* ledger.append_event(transaction, {
						agent_id: run.agent_id,
						causation_id: run.run_id,
						correlation_id: run.run_id,
						group_id: run.group_id,
						payload: {
							action: "started",
							attempt: run.attempt,
							group_id: run.group_id,
							node_id: run.run_id,
							node_type: "agent_run",
							state: "running",
							type: "orchestration.graph.lifecycle",
						},
						raw_origin,
						run_id: run.run_id,
						thread_id: group.thread_id,
					});
					const projected_run = yield* Schema.decodeUnknownEffect(AgentRun)({
						agent_id: updated_run.agent_id,
						assignment_id: updated_run.assignment_id,
						attempt: updated_run.attempt,
						created_at: updated_run.created_at,
						engine_id: updated_run.engine_id,
						group_id: updated_run.group_id,
						last_observation_sequence: updated_run.last_observation_sequence,
						native_identity,
						native_thread_id,
						profile: updated_run.profile,
						raw_origin,
						run_id: updated_run.run_id,
						state: "running",
						updated_at: updated_run.updated_at,
					}).pipe(
						Effect.mapError(
							() =>
								new AgentGraphInvalid({
									message: `Agent run ${run_id} is invalid`,
								}),
						),
					);

					return { events: [event], run: projected_run };
				}),
			);

			yield* ledger.publish_events(result.events);

			return result;
		}).pipe(Effect.mapError(normalize_graph_error));

	const fail_run_start = (run_id: string, instance_id: string, failure: string) =>
		Effect.gen(function* () {
			const events = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [run] = yield* transaction
						.select()
						.from(AgentRuns)
						.where(
							and(
								eq(AgentRuns.run_id, run_id),
								eq(AgentRuns.owner_instance_id, instance_id),
								eq(AgentRuns.dispatch_status, "dispatching"),
							),
						)
						.limit(1);

					if (!run) {
						return [];
					}

					const [group] = yield* transaction
						.select({ thread_id: OrchestrationGroups.thread_id })
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, run.group_id))
						.limit(1);

					if (!group) {
						return [];
					}

					const failed_at = yield* metadata.Now;

					yield* transaction
						.update(Assignments)
						.set({
							heartbeat_json: JSON.stringify({
								blocked_reason: failure.slice(0, 240),
								confidence: 0,
								current_action: "Waiting for a retry",
								short_description: "Engine start failed",
								updated_at: failed_at,
							}),
						})
						.where(eq(Assignments.assignment_id, run.assignment_id));

					return yield* transitions.transition_terminal_run(transaction, {
						causation_id: `start_failed:${run_id}`,
						correlation_id: run_id,
						run,
						state: "failed",
						thread_id: group.thread_id,
					});
				}),
			);

			yield* ledger.publish_events(events);

			return events;
		}).pipe(Effect.mapError(normalize_graph_error));

	const record_observation = (observation: EngineObservation) =>
		Effect.gen(function* () {
			const events = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const inserted = yield* raw_observations.append_raw_observation(
						transaction,
						observation,
					);

					if (!inserted) {
						return [];
					}

					const [run] = yield* transaction
						.select()
						.from(AgentRuns)
						.where(eq(AgentRuns.run_id, observation.artisan_run_id))
						.limit(1);

					if (
						!run ||
						observation.sequence <= run.last_observation_sequence ||
						is_terminal_state(run.state) ||
						run.dispatch_status !== "active"
					) {
						return [];
					}

					const [group] = yield* transaction
						.select({ thread_id: OrchestrationGroups.thread_id })
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, run.group_id))
						.limit(1);

					if (!group) {
						return [];
					}

					const updated_at = yield* metadata.Now;
					const advanced = yield* transaction
						.update(AgentRuns)
						.set({ last_observation_sequence: observation.sequence, updated_at })
						.where(
							and(
								eq(AgentRuns.run_id, run.run_id),
								eq(
									AgentRuns.last_observation_sequence,
									run.last_observation_sequence,
								),
							),
						)
						.returning({ run_id: AgentRuns.run_id });

					if (advanced.length !== 1) {
						return [];
					}

					const raw_origin = {
						provider: observation.raw.engine_id,
						reference: String(observation.raw.native_id ?? observation.observation_id),
					} satisfies RawOrigin;

					if (observation._tag === "run_terminal") {
						return yield* transitions.transition_terminal_run(transaction, {
							causation_id: observation.observation_id,
							correlation_id: run.run_id,
							raw_origin,
							run,
							state: terminal_state_from_engine(observation.state),
							thread_id: group.thread_id,
						});
					}

					if (observation._tag === "run_state") {
						if (run.state === "summarized") {
							return [];
						}

						const state = observation.state === "waiting" ? "waiting" : "running";

						yield* transaction
							.update(AgentRuns)
							.set({ state, updated_at })
							.where(
								and(
									eq(AgentRuns.run_id, run.run_id),
									eq(AgentRuns.dispatch_status, "active"),
								),
							);
						yield* transaction
							.update(Assignments)
							.set({ state, updated_at })
							.where(
								and(
									eq(Assignments.assignment_id, run.assignment_id),
									eq(Assignments.active_run_id, run.run_id),
									ne(Assignments.state, "blocked"),
								),
							);

						return [
							yield* ledger.append_event(transaction, {
								agent_id: run.agent_id,
								causation_id: observation.observation_id,
								correlation_id: run.run_id,
								group_id: run.group_id,
								payload: {
									action: "provider_state",
									attempt: run.attempt,
									group_id: run.group_id,
									node_id: run.run_id,
									node_type: "agent_run",
									state,
									type: "orchestration.graph.lifecycle",
								},
								raw_origin,
								run_id: run.run_id,
								thread_id: group.thread_id,
							}),
						];
					}

					if (observation._tag !== "agent_message_completed") {
						return [];
					}

					const artifact_id = yield* metadata.MakeId("message");
					const artifact = {
						artifact_id,
						assignment_id: run.assignment_id,
						content: observation.message,
						created_at: updated_at,
						group_id: run.group_id,
						kind: "summary" as const,
						label: "Agent summary",
						raw_origin,
						run_id: run.run_id,
					};

					yield* transaction.insert(OrchestrationArtifacts).values({
						...artifact,
						raw_origin_json: JSON.stringify(raw_origin),
						uri: null,
					});
					yield* transaction
						.update(AgentRuns)
						.set({ state: "summarized", updated_at })
						.where(eq(AgentRuns.run_id, run.run_id));
					yield* transaction
						.update(Assignments)
						.set({ state: "summarized", updated_at })
						.where(
							and(
								eq(Assignments.assignment_id, run.assignment_id),
								eq(Assignments.active_run_id, run.run_id),
							),
						);

					return [
						yield* ledger.append_event(transaction, {
							agent_id: run.agent_id,
							causation_id: observation.observation_id,
							correlation_id: run.run_id,
							group_id: run.group_id,
							payload: {
								artifact,
								group_id: run.group_id,
								type: "artifact.recorded",
							},
							raw_origin,
							run_id: run.run_id,
							thread_id: group.thread_id,
						}),
						yield* ledger.append_event(transaction, {
							agent_id: run.agent_id,
							causation_id: observation.observation_id,
							correlation_id: run.run_id,
							group_id: run.group_id,
							payload: {
								action: "summary_recorded",
								attempt: run.attempt,
								group_id: run.group_id,
								node_id: run.assignment_id,
								node_type: "assignment",
								state: "summarized",
								type: "orchestration.graph.lifecycle",
							},
							raw_origin,
							run_id: run.run_id,
							thread_id: group.thread_id,
						}),
					];
				}),
			);

			yield* ledger.publish_events(events);

			return events;
		}).pipe(Effect.mapError(normalize_graph_error));

	const record_closed = (run_id: string, state: EngineRunTerminalState) =>
		Effect.gen(function* () {
			const events = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [run] = yield* transaction
						.select()
						.from(AgentRuns)
						.where(eq(AgentRuns.run_id, run_id))
						.limit(1);

					if (!run || run.dispatch_status !== "active" || is_terminal_state(run.state)) {
						return [];
					}

					const [group] = yield* transaction
						.select({ thread_id: OrchestrationGroups.thread_id })
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, run.group_id))
						.limit(1);

					return group
						? yield* transitions.transition_terminal_run(transaction, {
								causation_id: `closed:${run_id}`,
								correlation_id: run_id,
								run,
								state: terminal_state_from_engine(state),
								thread_id: group.thread_id,
							})
						: [];
				}),
			);

			yield* ledger.publish_events(events);

			return events;
		}).pipe(Effect.mapError(normalize_graph_error));

	const recover = (instance_id: string) =>
		Effect.gen(function* () {
			const events = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const stranded = yield* transaction
						.select()
						.from(AgentRuns)
						.where(
							and(
								inArray(AgentRuns.dispatch_status, ["dispatching", "active"]),
								ne(AgentRuns.owner_instance_id, instance_id),
							),
						)
						.orderBy(asc(AgentRuns.created_at), asc(AgentRuns.run_id));
					const recovered: Array<EventEnvelope> = [];

					for (const run of stranded) {
						const [group] = yield* transaction
							.select({ thread_id: OrchestrationGroups.thread_id })
							.from(OrchestrationGroups)
							.where(eq(OrchestrationGroups.group_id, run.group_id))
							.limit(1);

						if (group) {
							recovered.push(
								...(yield* transitions.transition_terminal_run(transaction, {
									causation_id: `recovered:${run.run_id}`,
									correlation_id: run.run_id,
									run,
									state: "failed",
									thread_id: group.thread_id,
								})),
							);
						}
					}

					return recovered;
				}),
			);

			yield* ledger.publish_events(events);

			return events;
		}).pipe(Effect.mapError(normalize_graph_error));

	return { activate_run, claim_run, fail_run_start, record_closed, record_observation, recover };
}
