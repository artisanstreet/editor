import { and, eq, isNull } from "drizzle-orm";
import { Effect } from "effect";

import type {
	EngineSubagentObservation,
	EngineSubagentTranscriptObservation,
} from "@artisan/engines";
import type {
	AssignmentHeartbeat,
	EventEnvelope,
	OrchestrationLifecycleState,
	RawOrigin,
} from "@artisan/protocol";

import {
	AgentInstances,
	AgentRuns,
	Assignments,
	NativeSubagentBindings,
	NativeSubagentObservationInbox,
	OrchestrationGraphEdges,
	OrchestrationGroups,
	OrchestrationRuns,
} from "../../persistence/tables";
import type { DatabaseClient } from "../../persistence/database";
import { EnsureThread, EnsureTurn, UpsertItem } from "../../conversation/projection/entities";
import { item_base, turn_base } from "../../conversation/projection/domain";
import { make_native_subagent_transcripts } from "./native-subagent-transcripts";
import { RecoverNativeSubagents } from "./native-subagent-recovery";
import { MakeTerminalTranscriptConsumption } from "./native-subagent-terminal-transcripts";
import { ChooseAvailableAgentName } from "./agent-name-allocation";
import { ApplyEngineObservation } from "../../conversation/index";
import { normalize_graph_error } from "../agent-graph-model";
import { compact_status_text, is_terminal_state, type GraphContext } from "./graph-context";
import type { GraphLedger } from "./graph-ledger";

type PendingNativeSubagent = typeof NativeSubagentObservationInbox.$inferSelect;

const state_from_native = (state: string): OrchestrationLifecycleState =>
	state === "completed"
		? "complete"
		: state === "failed"
			? "failed"
			: state === "interrupted"
				? "stopped"
				: state === "waiting"
					? "waiting"
					: "running";

const terminal_state_from_root = (status: string): OrchestrationLifecycleState =>
	status === "completed" ? "complete" : status === "failed" ? "failed" : "stopped";

const conversation_lifecycle_for = (state: OrchestrationLifecycleState) =>
	state === "complete"
		? "completed"
		: state === "stopped"
			? "cancelled"
			: state === "failed"
				? "failed"
				: state === "waiting" || state === "joining"
					? "waiting"
					: "active";

const raw_origin_for = (observation: PendingNativeSubagent): RawOrigin => ({
	provider: observation.engine_id,
	reference: observation.native_id ?? observation.observation_id,
});

const visible_activity_for = (activity: string) => {
	switch (activity.trim().toLowerCase()) {
		case "spawn_agent":
		case "started":
			return "Starting delegated work";
		case "send_input":
			return "Following up on delegated work";
		case "wait":
			return "Waiting for related agents";
		case "resume_agent":
			return "Resuming delegated work";
		case "close_agent":
			return "Finishing delegated work";
		default:
			return activity;
	}
};

const heartbeat_for = (activity: string | null, updated_at: string) => {
	if (activity === null || activity.trim().length === 0) {
		return Effect.succeed<AssignmentHeartbeat | undefined>(undefined);
	}
	const visible_activity = visible_activity_for(activity);

	return Effect.all({
		current_action: compact_status_text(visible_activity, "native subagent activity", 240),
		short_description: compact_status_text(visible_activity, "native subagent activity", 160),
	}).pipe(
		Effect.map(
			(status): AssignmentHeartbeat => ({
				confidence: 0.5,
				current_action: status.current_action,
				short_description: status.short_description,
				updated_at,
			}),
		),
		// Provider labels are provenance. Unsafe or overlong labels never become UI copy.
		Effect.catch(() => Effect.succeed(undefined)),
	);
};

/** Adopts provider-observed children without making them dispatchable Artisan work. */
export function make_native_subagents(context: GraphContext, ledger: GraphLedger) {
	/** Direct delegation owns one stable coordinator row; nested work belongs in its parent transcript. */
	const ProjectDirectDelegation = (
		transaction: DatabaseClient,
		input: {
			readonly agent_id: string;
			readonly display_name: string;
			readonly root: typeof OrchestrationRuns.$inferSelect;
			readonly state: OrchestrationLifecycleState;
			readonly updated_at: string;
			readonly observation_id: string;
		},
	) =>
		Effect.gen(function* () {
			const context = {
				agent_id: input.root.agent_id,
				occurred_at: input.updated_at,
				run_id: input.root.run_id,
				thread_id: input.root.thread_id,
			};
			yield* EnsureThread(transaction, input.root.thread_id, input.updated_at);
			yield* EnsureTurn(
				transaction,
				input.root.thread_id,
				turn_base(`run:${input.root.run_id}`, context, "active", input.observation_id),
				{ observed_at: input.updated_at },
			);
			return yield* UpsertItem(
				transaction,
				input.root.thread_id,
				{
					...item_base(
						`activity:subagent:${input.root.run_id}:${input.agent_id}`,
						`run:${input.root.run_id}`,
						context,
						conversation_lifecycle_for(input.state),
						input.observation_id,
					),
					kind: "subagent",
					label: "Talked to subagent",
					status: conversation_lifecycle_for(input.state),
					subagent: { agent_id: input.agent_id, display_name: input.display_name },
					type: "activity",
				},
				{ observed_at: input.updated_at },
			);
		});

	const ProjectChildLifecycle = (
		transaction: DatabaseClient,
		input: {
			readonly agent_id: string;
			readonly native_thread_id: string;
			readonly observation_id: string;
			readonly root: typeof OrchestrationRuns.$inferSelect;
			readonly run_id: string;
			readonly state: OrchestrationLifecycleState;
			readonly updated_at: string;
		},
	) => {
		const turn_id = `native:${input.native_thread_id}`;
		const base = {
			artisan_run_id: input.run_id,
			native_thread_id: input.native_thread_id,
			observation_id: `${input.observation_id}:child-lifecycle`,
			raw: {
				engine_id: input.root.engine_id,
				frame: { state: input.state },
				transport: "native-subagent-projection",
			},
			sequence: 0,
		};
		const context = {
			agent_id: input.agent_id,
			occurred_at: input.updated_at,
			parent_run_id: input.root.run_id,
			run_id: input.run_id,
			thread_id: input.root.thread_id,
		};
		const turn_state =
			input.state === "waiting"
				? "waiting"
				: input.state === "complete"
					? "completed"
					: input.state === "failed"
						? "failed"
						: input.state === "stopped"
							? "cancelled"
							: "started";
		return ApplyEngineObservation(
			transaction,
			{ ...base, _tag: "turn_state", state: turn_state, turn_id },
			context,
		);
	};

	const ProjectNestedDelegation = (
		transaction: DatabaseClient,
		input: {
			readonly child_agent_id: string;
			readonly child_display_name: string;
			readonly observation_id: string;
			readonly parent_agent_id: string;
			readonly parent_run_id: string;
			readonly root: typeof OrchestrationRuns.$inferSelect;
			readonly state: OrchestrationLifecycleState;
			readonly updated_at: string;
		},
	) =>
		Effect.gen(function* () {
			const context = {
				agent_id: input.parent_agent_id,
				occurred_at: input.updated_at,
				parent_run_id: input.root.run_id,
				run_id: input.parent_run_id,
				thread_id: input.root.thread_id,
			};
			yield* EnsureThread(transaction, input.root.thread_id, input.updated_at);
			yield* EnsureTurn(
				transaction,
				input.root.thread_id,
				turn_base(`run:${input.parent_run_id}`, context, "active", input.observation_id),
				{ observed_at: input.updated_at },
			);
			return yield* UpsertItem(
				transaction,
				input.root.thread_id,
				{
					...item_base(
						`activity:subagent:${input.parent_run_id}:${input.child_agent_id}`,
						`run:${input.parent_run_id}`,
						context,
						conversation_lifecycle_for(input.state),
						input.observation_id,
					),
					kind: "subagent",
					label: "Talked to subagent",
					status: conversation_lifecycle_for(input.state),
					subagent: {
						agent_id: input.child_agent_id,
						display_name: input.child_display_name,
					},
					type: "activity",
				},
				{ observed_at: input.updated_at },
			);
		});

	const ReconcileRoot = (root_run_id: string) =>
		context.database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [binding] = yield* transaction
						.select()
						.from(NativeSubagentBindings)
						.where(eq(NativeSubagentBindings.root_run_id, root_run_id))
						.limit(1);
					if (!binding) return [] as ReadonlyArray<EventEnvelope>;

					const [root] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, root_run_id))
						.limit(1);
					if (!root) return [] as ReadonlyArray<EventEnvelope>;

					const [group] = yield* transaction
						.select()
						.from(OrchestrationGroups)
						.where(eq(OrchestrationGroups.group_id, binding.group_id))
						.limit(1);
					if (!group || is_terminal_state(group.state)) {
						return [] as ReadonlyArray<EventEnvelope>;
					}

					const children = yield* transaction
						.select()
						.from(NativeSubagentBindings)
						.where(eq(NativeSubagentBindings.group_id, binding.group_id));
					const has_active_child = children.some(
						(child) => !is_terminal_state(child.state),
					);
					const root_active = ["queued", "running", "waiting"].includes(root.status);
					const terminal_root_state = terminal_state_from_root(root.status);
					const updated_at = yield* context.metadata.Now;
					const events: Array<EventEnvelope> = [];

					/**
					 * A provider can finish the root without ever delivering a terminal
					 * frame for each child it exposed. The root is the only authority left
					 * at that point: retaining those rows as running makes the aggregate
					 * join forever and resurrects a finished thread as Working. Do not
					 * invent successful child outcomes; `stopped` records the missing
					 * child terminal evidence while closing every visible projection.
					 */
					if (!root_active && has_active_child) {
						for (const child of children) {
							if (is_terminal_state(child.state)) continue;

							yield* transaction
								.update(NativeSubagentBindings)
								.set({ state: "stopped", updated_at })
								.where(eq(NativeSubagentBindings.binding_id, child.binding_id));
							yield* transaction
								.update(AgentRuns)
								.set({ completed_at: updated_at, state: "stopped", updated_at })
								.where(eq(AgentRuns.run_id, child.run_id));
							yield* transaction
								.update(Assignments)
								.set({ state: "stopped", updated_at })
								.where(
									and(
										eq(Assignments.assignment_id, child.assignment_id),
										eq(Assignments.active_run_id, child.run_id),
									),
								);

							const [agent] = yield* transaction
								.select({ display_name: AgentInstances.display_name })
								.from(AgentInstances)
								.where(eq(AgentInstances.agent_id, child.agent_id))
								.limit(1);
							if (agent !== undefined) {
								if (child.parent_native_thread_id === root.native_thread_id) {
									yield* ProjectDirectDelegation(transaction, {
										agent_id: child.agent_id,
										display_name: agent.display_name,
										observation_id: `native-root-terminal:${root.run_id}:${child.binding_id}`,
										root,
										state: "stopped",
										updated_at,
									});
								} else {
									const [parent] = yield* transaction
										.select({
											agent_id: NativeSubagentBindings.agent_id,
											run_id: NativeSubagentBindings.run_id,
										})
										.from(NativeSubagentBindings)
										.where(
											and(
												eq(NativeSubagentBindings.root_run_id, root.run_id),
												eq(
													NativeSubagentBindings.agent_native_thread_id,
													child.parent_native_thread_id,
												),
											),
										)
										.limit(1);
									if (parent !== undefined) {
										yield* ProjectNestedDelegation(transaction, {
											child_agent_id: child.agent_id,
											child_display_name: agent.display_name,
											observation_id: `native-root-terminal:${root.run_id}:${child.binding_id}`,
											parent_agent_id: parent.agent_id,
											parent_run_id: parent.run_id,
											root,
											state: "stopped",
											updated_at,
										});
									}
								}
							}
							yield* ProjectChildLifecycle(transaction, {
								agent_id: child.agent_id,
								native_thread_id: child.agent_native_thread_id,
								observation_id: `native-root-terminal:${root.run_id}:${child.binding_id}`,
								root,
								run_id: child.run_id,
								state: "stopped",
								updated_at,
							});
							events.push(
								yield* ledger.append_event(transaction, {
									agent_id: child.agent_id,
									causation_id: `native-root-terminal:${root.run_id}:${child.binding_id}`,
									correlation_id: root.run_id,
									group_id: child.group_id,
									payload: {
										action: "provider root terminalized subagent",
										attempt: 1,
										group_id: child.group_id,
										node_id: child.run_id,
										node_type: "agent_run",
										state: "stopped",
										type: "orchestration.graph.lifecycle",
									},
									raw_origin: {
										provider: child.engine_id,
										reference: child.binding_id,
									},
									run_id: child.run_id,
									thread_id: root.thread_id,
								}),
							);
						}
					}
					const state: OrchestrationLifecycleState = root_active
						? "running"
						: terminal_root_state;
					if (group.state === state) return events;

					yield* transaction
						.update(OrchestrationGroups)
						.set({ state, updated_at })
						.where(eq(OrchestrationGroups.group_id, binding.group_id));

					events.push(
						yield* ledger.append_event(transaction, {
							agent_id: group.coordinator_agent_id,
							causation_id: `native-root:${root.run_id}:${state}`,
							correlation_id: root.run_id,
							group_id: group.group_id,
							payload: {
								action: "provider root lifecycle reconciled",
								group_id: group.group_id,
								node_id: group.group_id,
								node_type: "orchestration_group",
								state,
								type: "orchestration.graph.lifecycle",
							},
							raw_origin: {
								provider: binding.engine_id,
								reference: binding.binding_id,
							},
							thread_id: root.thread_id,
						}),
					);

					return events;
				}),
			)
			.pipe(
				Effect.tap(ledger.publish_events),
				Effect.asVoid,
				Effect.mapError(normalize_graph_error),
			);

	const RecordPending = (observation_id: string, name_bank: ReadonlyArray<string>) =>
		context.database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [observation] = yield* transaction
						.select()
						.from(NativeSubagentObservationInbox)
						.where(
							and(
								eq(NativeSubagentObservationInbox.observation_id, observation_id),
								isNull(NativeSubagentObservationInbox.processed_at),
							),
						)
						.limit(1);
					if (!observation) return [] as ReadonlyArray<EventEnvelope>;

					const [root] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, observation.root_run_id))
						.limit(1);
					// Leave the inbox row pending if its root has not become visible yet.
					if (!root) return [] as ReadonlyArray<EventEnvelope>;

					const now = yield* context.metadata.Now;
					if (!["queued", "running", "waiting"].includes(root.status)) {
						yield* transaction
							.delete(NativeSubagentObservationInbox)
							.where(
								and(
									eq(
										NativeSubagentObservationInbox.observation_id,
										observation.observation_id,
									),
									isNull(NativeSubagentObservationInbox.processed_at),
								),
							);
						return [] as ReadonlyArray<EventEnvelope>;
					}
					const heartbeat = yield* heartbeat_for(observation.activity, now);
					const raw_origin = raw_origin_for(observation);
					const next_state = state_from_native(observation.state);
					const MarkProcessed = transaction
						.delete(NativeSubagentObservationInbox)
						.where(
							and(
								eq(
									NativeSubagentObservationInbox.observation_id,
									observation.observation_id,
								),
								isNull(NativeSubagentObservationInbox.processed_at),
							),
						);

					const [existing] = yield* transaction
						.select()
						.from(NativeSubagentBindings)
						.where(
							and(
								eq(NativeSubagentBindings.engine_id, observation.engine_id),
								eq(NativeSubagentBindings.root_run_id, root.run_id),
								eq(
									NativeSubagentBindings.agent_native_thread_id,
									observation.agent_native_thread_id,
								),
							),
						)
						.limit(1);

					if (existing) {
						const [existing_run] = yield* transaction
							.select({
								last_observation_sequence: AgentRuns.last_observation_sequence,
							})
							.from(AgentRuns)
							.where(eq(AgentRuns.run_id, existing.run_id))
							.limit(1);
						if (
							is_terminal_state(existing.state) ||
							(existing_run !== undefined &&
								existing_run.last_observation_sequence >= observation.sequence)
						) {
							yield* MarkProcessed;
							return [] as ReadonlyArray<EventEnvelope>;
						}
						const [existing_group] = yield* transaction
							.select()
							.from(OrchestrationGroups)
							.where(eq(OrchestrationGroups.group_id, existing.group_id))
							.limit(1);
						const [resolved_parent] = yield* transaction
							.select({
								agent_id: NativeSubagentBindings.agent_id,
								assignment_id: NativeSubagentBindings.assignment_id,
								run_id: NativeSubagentBindings.run_id,
							})
							.from(NativeSubagentBindings)
							.where(
								and(
									eq(NativeSubagentBindings.engine_id, observation.engine_id),
									eq(NativeSubagentBindings.root_run_id, root.run_id),
									eq(
										NativeSubagentBindings.agent_native_thread_id,
										observation.parent_native_thread_id,
									),
								),
							)
							.limit(1);
						const resolved_parent_node_id =
							resolved_parent?.assignment_id ??
							(observation.parent_native_thread_id === root.native_thread_id
								? existing_group?.coordinator_agent_id
								: undefined);
						if (
							resolved_parent_node_id !== undefined &&
							resolved_parent_node_id !== existing.assignment_id &&
							existing_group !== undefined &&
							!is_terminal_state(existing_group.state)
						) {
							yield* transaction
								.update(OrchestrationGraphEdges)
								.set({ from_node_id: resolved_parent_node_id })
								.where(
									and(
										eq(OrchestrationGraphEdges.group_id, existing.group_id),
										eq(OrchestrationGraphEdges.kind, "delegation"),
										eq(
											OrchestrationGraphEdges.to_node_id,
											existing.assignment_id,
										),
									),
								);
						}

						yield* transaction
							.update(NativeSubagentBindings)
							.set({
								activity: observation.activity ?? existing.activity,
								agent_path: observation.agent_path ?? existing.agent_path,
								raw_origin_json: JSON.stringify(raw_origin),
								parent_native_thread_id: observation.parent_native_thread_id,
								state: next_state,
								turn_id: observation.turn_id ?? existing.turn_id,
								updated_at: now,
							})
							.where(eq(NativeSubagentBindings.binding_id, existing.binding_id));
						yield* transaction
							.update(AgentRuns)
							.set({
								...(is_terminal_state(next_state) ? { completed_at: now } : {}),
								last_observation_sequence: observation.sequence,
								state: next_state,
								updated_at: now,
							})
							.where(eq(AgentRuns.run_id, existing.run_id));
						yield* transaction
							.update(Assignments)
							.set({
								...(heartbeat === undefined
									? {}
									: { heartbeat_json: JSON.stringify(heartbeat) }),
								state: next_state,
								updated_at: now,
							})
							.where(eq(Assignments.assignment_id, existing.assignment_id));
						if (observation.parent_native_thread_id === root.native_thread_id) {
							const [agent] = yield* transaction
								.select({ display_name: AgentInstances.display_name })
								.from(AgentInstances)
								.where(eq(AgentInstances.agent_id, existing.agent_id))
								.limit(1);
							if (agent !== undefined) {
								yield* ProjectDirectDelegation(transaction, {
									agent_id: existing.agent_id,
									display_name: agent.display_name,
									observation_id: observation.observation_id,
									root,
									state: next_state,
									updated_at: now,
								});
							}
						} else if (resolved_parent !== undefined) {
							const [agent] = yield* transaction
								.select({ display_name: AgentInstances.display_name })
								.from(AgentInstances)
								.where(eq(AgentInstances.agent_id, existing.agent_id))
								.limit(1);
							if (agent !== undefined) {
								yield* ProjectNestedDelegation(transaction, {
									child_agent_id: existing.agent_id,
									child_display_name: agent.display_name,
									observation_id: observation.observation_id,
									parent_agent_id: resolved_parent.agent_id,
									parent_run_id: resolved_parent.run_id,
									root,
									state: next_state,
									updated_at: now,
								});
							}
						}
						yield* ProjectChildLifecycle(transaction, {
							agent_id: existing.agent_id,
							native_thread_id: observation.agent_native_thread_id,
							observation_id: observation.observation_id,
							root,
							run_id: existing.run_id,
							state: next_state,
							updated_at: now,
						});
						yield* MarkProcessed;

						return [
							yield* ledger.append_event(transaction, {
								agent_id: existing.agent_id,
								causation_id: observation.observation_id,
								correlation_id: root.run_id,
								group_id: existing.group_id,
								payload: {
									action: "provider observed subagent",
									attempt: 1,
									group_id: existing.group_id,
									node_id: existing.run_id,
									node_type: "agent_run",
									state: next_state,
									type: "orchestration.graph.lifecycle",
								},
								raw_origin,
								run_id: existing.run_id,
								thread_id: root.thread_id,
							}),
						];
					}

					const [root_binding] = yield* transaction
						.select()
						.from(NativeSubagentBindings)
						.where(
							and(
								eq(NativeSubagentBindings.engine_id, observation.engine_id),
								eq(NativeSubagentBindings.root_run_id, root.run_id),
							),
						)
						.limit(1);
					const group_id =
						root_binding?.group_id ?? (yield* context.metadata.MakeId("agent"));
					const [group] = root_binding
						? yield* transaction
								.select()
								.from(OrchestrationGroups)
								.where(eq(OrchestrationGroups.group_id, group_id))
								.limit(1)
						: [];
					if (group !== undefined && is_terminal_state(group.state)) {
						yield* MarkProcessed;
						return [] as ReadonlyArray<EventEnvelope>;
					}
					const coordinator_agent_id =
						group?.coordinator_agent_id ?? (yield* context.metadata.MakeId("agent"));
					const existing_agents = yield* transaction
						.select({
							display_name: AgentInstances.display_name,
						})
						.from(AgentInstances)
						.innerJoin(
							OrchestrationGroups,
							eq(OrchestrationGroups.group_id, AgentInstances.group_id),
						)
						.where(eq(OrchestrationGroups.thread_id, root.thread_id));
					const display_name = yield* ChooseAvailableAgentName(
						name_bank,
						existing_agents.map((agent) => agent.display_name),
					);
					const [parent_binding] = yield* transaction
						.select()
						.from(NativeSubagentBindings)
						.where(
							and(
								eq(NativeSubagentBindings.engine_id, observation.engine_id),
								eq(NativeSubagentBindings.root_run_id, root.run_id),
								eq(
									NativeSubagentBindings.agent_native_thread_id,
									observation.parent_native_thread_id,
								),
							),
						)
						.limit(1);
					const parent_node_id = parent_binding?.assignment_id ?? coordinator_agent_id;
					const agent_id = yield* context.metadata.MakeId("agent");
					const assignment_id = yield* context.metadata.MakeId("agent");
					const run_id = yield* context.metadata.MakeId("run");
					const binding_id = yield* context.metadata.MakeId("agent");

					if (!group) {
						const root_active = ["queued", "running", "waiting"].includes(root.status);
						yield* transaction.insert(OrchestrationGroups).values({
							coordinator_agent_id,
							created_at: now,
							group_id,
							journal_sequence: 0,
							max_concurrency: 1,
							state: root_active
								? "running"
								: is_terminal_state(next_state)
									? terminal_state_from_root(root.status)
									: "joining",
							thread_id: root.thread_id,
							updated_at: now,
							version: 1,
						});
					} else if (group.max_concurrency < existing_agents.length) {
						yield* transaction
							.update(OrchestrationGroups)
							.set({ max_concurrency: existing_agents.length, updated_at: now })
							.where(eq(OrchestrationGroups.group_id, group_id));
					}

					yield* transaction.insert(AgentInstances).values([
						...(group
							? []
							: [
									{
										agent_id: coordinator_agent_id,
										created_at: now,
										display_name: "Coordinator",
										group_id,
										role: "coordinator",
										updated_at: now,
									},
								]),
						{
							agent_id,
							created_at: now,
							display_name,
							group_id,
							role: "worker",
							updated_at: now,
						},
					]);
					yield* transaction.insert(Assignments).values({
						active_run_id: run_id,
						agent_id,
						assignment_id,
						created_at: now,
						current_attempt: 1,
						engine_id: root.engine_id,
						expected_result: "Provider-native subagent result",
						group_id,
						heartbeat_json: heartbeat === undefined ? null : JSON.stringify(heartbeat),
						instructions: observation.activity ?? "Provider-native delegated work",
						max_attempts: 1,
						parent_node_id,
						permission_policy_json: JSON.stringify({
							approval: "on_request",
							network_access: false,
							write_access: false,
						}),
						profile: "provider-native",
						role: "worker",
						scope_json: JSON.stringify({
							kind: "custom",
							value: "Provider-native delegated work",
							write_access: false,
						}),
						state: next_state,
						summary_contract: "Return provider-native result to coordinator",
						updated_at: now,
						workspace_json: JSON.stringify({
							isolation: "shared",
							working_directory: root.working_directory,
							workspace_id: root.thread_id,
						}),
					});
					yield* transaction.insert(AgentRuns).values({
						agent_id,
						assignment_id,
						attempt: 1,
						completed_at: is_terminal_state(next_state) ? now : null,
						created_at: now,
						dispatch_status: "observed",
						engine_id: root.engine_id,
						execution_origin: "provider_observed",
						group_id,
						last_observation_sequence: observation.sequence,
						native_identity_json: JSON.stringify({
							thread_id: observation.agent_native_thread_id,
						}),
						native_resume_json: null,
						native_thread_id: observation.agent_native_thread_id,
						owner_instance_id: null,
						profile: "provider-native",
						raw_origin_json: JSON.stringify(raw_origin),
						run_id,
						state: next_state,
						updated_at: now,
					});
					yield* transaction.insert(OrchestrationGraphEdges).values({
						dispatch_dependency: 0,
						edge_id: yield* context.metadata.MakeId("agent"),
						from_node_id: parent_node_id,
						group_id,
						kind: "delegation",
						to_node_id: assignment_id,
					});
					yield* transaction.insert(NativeSubagentBindings).values({
						activity: observation.activity,
						agent_id,
						agent_native_thread_id: observation.agent_native_thread_id,
						agent_path: observation.agent_path,
						assignment_id,
						binding_id,
						created_at: now,
						engine_id: observation.engine_id,
						group_id,
						parent_native_thread_id: observation.parent_native_thread_id,
						raw_origin_json: JSON.stringify(raw_origin),
						root_run_id: root.run_id,
						run_id,
						state: next_state,
						turn_id: observation.turn_id,
						updated_at: now,
					});
					const awaiting_children = yield* transaction
						.select({
							agent_id: NativeSubagentBindings.agent_id,
							assignment_id: NativeSubagentBindings.assignment_id,
							run_id: NativeSubagentBindings.run_id,
						})
						.from(NativeSubagentBindings)
						.where(
							and(
								eq(NativeSubagentBindings.engine_id, observation.engine_id),
								eq(NativeSubagentBindings.root_run_id, root.run_id),
								eq(
									NativeSubagentBindings.parent_native_thread_id,
									observation.agent_native_thread_id,
								),
							),
						);
					for (const child of awaiting_children) {
						if (child.assignment_id === assignment_id) continue;
						yield* transaction
							.update(OrchestrationGraphEdges)
							.set({ from_node_id: assignment_id })
							.where(
								and(
									eq(OrchestrationGraphEdges.group_id, group_id),
									eq(OrchestrationGraphEdges.kind, "delegation"),
									eq(OrchestrationGraphEdges.to_node_id, child.assignment_id),
								),
							);
					}
					if (observation.parent_native_thread_id === root.native_thread_id) {
						yield* ProjectDirectDelegation(transaction, {
							agent_id,
							display_name,
							observation_id: observation.observation_id,
							root,
							state: next_state,
							updated_at: now,
						});
					} else if (parent_binding !== undefined) {
						yield* ProjectNestedDelegation(transaction, {
							child_agent_id: agent_id,
							child_display_name: display_name,
							observation_id: observation.observation_id,
							parent_agent_id: parent_binding.agent_id,
							parent_run_id: parent_binding.run_id,
							root,
							state: next_state,
							updated_at: now,
						});
					}
					yield* ProjectChildLifecycle(transaction, {
						agent_id,
						native_thread_id: observation.agent_native_thread_id,
						observation_id: observation.observation_id,
						root,
						run_id,
						state: next_state,
						updated_at: now,
					});
					yield* MarkProcessed;

					return [
						yield* ledger.append_event(transaction, {
							agent_id,
							causation_id: observation.observation_id,
							correlation_id: root.run_id,
							group_id,
							payload: {
								action: "provider discovered subagent",
								attempt: 1,
								group_id,
								node_id: run_id,
								node_type: "agent_run",
								state: next_state,
								type: "orchestration.graph.lifecycle",
							},
							raw_origin,
							run_id,
							thread_id: root.thread_id,
						}),
					];
				}),
			)
			.pipe(Effect.tap(ledger.publish_events), Effect.mapError(normalize_graph_error));

	const transcripts = make_native_subagent_transcripts(context);
	const terminal_transcripts = MakeTerminalTranscriptConsumption(context);
	const Record = (observation: EngineSubagentObservation | EngineSubagentTranscriptObservation) =>
		(observation._tag === "subagent_transcript"
			? terminal_transcripts
					.Consume(observation.observation_id)
					.pipe(
						Effect.andThen(transcripts.RecordPending(observation.observation_id)),
						Effect.as([] as ReadonlyArray<EventEnvelope>),
					)
			: context.agent_name_catalog.Names.pipe(
					Effect.flatMap((name_bank) =>
						RecordPending(observation.observation_id, name_bank),
					),
					Effect.tap(() => transcripts.DrainRoot(observation.artisan_run_id)),
				)
		).pipe(Effect.mapError(normalize_graph_error));

	const Recover = RecoverNativeSubagents(context, {
		ConsumeTerminalTranscript: terminal_transcripts.Consume,
		ReconcileRoot,
		RecordPending,
		RecoverTranscripts: transcripts.Recover,
	}).pipe(Effect.mapError(normalize_graph_error));

	return { Record, ReconcileRoot, Recover };
}
