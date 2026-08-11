import { asc, eq } from "drizzle-orm";
import { Effect, Schema } from "effect";

import {
	AgentRun,
	Artifact,
	Assignment,
	AssignmentHeartbeat,
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	EventEnvelope,
	EventPayload,
	GraphEdge,
	Join,
	OrchestrationGraph,
	OrchestrationGroup,
	ProviderNativeIdentity,
	RawOrigin,
	type EventEnvelope as EventEnvelopeType,
	type OrchestrationGraph as OrchestrationGraphType,
} from "@artisan/protocol";

import {
	AgentInstances,
	AgentRuns,
	Assignments,
	JournalEvents,
	OrchestrationArtifacts,
	OrchestrationGraphEdges,
	OrchestrationGroups,
	OrchestrationJoins,
} from "../../persistence/tables";
import { AgentGraphInvalid, AgentGraphNotFound } from "../agent-graph-model";
import type { GraphContext, GraphTransaction } from "./graph-context";

export interface PersistedGraphCodecs {
	readonly decode_event_row: (
		row: typeof JournalEvents.$inferSelect,
	) => Effect.Effect<EventEnvelopeType, unknown>;
	readonly decode_json: <A, I, R>(
		schema: Schema.Codec<A, I, R>,
		json: string,
		context: string,
	) => Effect.Effect<A, unknown, R>;
	readonly build_graph: (
		transaction: GraphTransaction,
		group_id: string,
	) => Effect.Effect<OrchestrationGraphType, unknown>;
}

/** Owns JSON boundaries and projection reconstruction for persisted graph rows. */
export function make_persisted_graph_codecs(_context: GraphContext): PersistedGraphCodecs {
	const parse_json = (json: string, context: string) =>
		Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
			Effect.mapError(
				() => new AgentGraphInvalid({ message: `${context} contains invalid JSON` }),
			),
		);

	const decode_json = <A, I, R>(schema: Schema.Codec<A, I, R>, json: string, context: string) =>
		parse_json(json, context).pipe(
			Effect.flatMap((value) =>
				Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(value).pipe(
					Effect.mapError(
						() => new AgentGraphInvalid({ message: `${context} has an invalid shape` }),
					),
				),
			),
		);

	const decode_event_row = (row: typeof JournalEvents.$inferSelect) =>
		Effect.gen(function* () {
			const payload = yield* decode_json(
				EventPayload,
				row.payload_json,
				`Journal event ${row.event_id} payload`,
			);
			const raw_origin = row.raw_origin_json
				? yield* decode_json(
						RawOrigin,
						row.raw_origin_json,
						`Journal event ${row.event_id} raw origin`,
					)
				: undefined;

			return yield* Schema.decodeUnknownEffect(EventEnvelope, {
				onExcessProperty: "error",
			})({
				...(row.agent_id ? { agent_id: row.agent_id } : {}),
				causation_id: row.causation_id,
				correlation_id: row.correlation_id,
				journal_sequence: row.sequence,
				kind: "event",
				message_id: row.event_id,
				origin: row.origin,
				payload,
				protocol_version: 1,
				...(raw_origin ? { raw_origin } : {}),
				...(row.run_id ? { run_id: row.run_id } : {}),
				schema_version: row.schema_version,
				sequence: row.stream_sequence,
				sent_at: row.occurred_at,
				stream_id: row.stream_id,
				thread_id: row.thread_id,
			}).pipe(
				Effect.mapError(
					() =>
						new AgentGraphInvalid({
							message: `Journal event ${row.event_id} cannot be reconstructed`,
						}),
				),
			);
		});

	const build_graph = (transaction: GraphTransaction, group_id: string) =>
		Effect.gen(function* () {
			const [group_row] = yield* transaction
				.select()
				.from(OrchestrationGroups)
				.where(eq(OrchestrationGroups.group_id, group_id))
				.limit(1);

			if (!group_row) {
				return yield* new AgentGraphNotFound({
					id: group_id,
					resource: "orchestration_group",
				});
			}

			const agent_rows = yield* transaction
				.select()
				.from(AgentInstances)
				.where(eq(AgentInstances.group_id, group_id))
				.orderBy(asc(AgentInstances.created_at), asc(AgentInstances.agent_id));
			const assignment_rows = yield* transaction
				.select()
				.from(Assignments)
				.where(eq(Assignments.group_id, group_id))
				.orderBy(asc(Assignments.created_at), asc(Assignments.assignment_id));
			const run_rows = yield* transaction
				.select()
				.from(AgentRuns)
				.where(eq(AgentRuns.group_id, group_id))
				.orderBy(asc(AgentRuns.created_at), asc(AgentRuns.attempt), asc(AgentRuns.run_id));
			const join_rows = yield* transaction
				.select()
				.from(OrchestrationJoins)
				.where(eq(OrchestrationJoins.group_id, group_id))
				.orderBy(asc(OrchestrationJoins.created_at), asc(OrchestrationJoins.join_id));
			const edge_rows = yield* transaction
				.select()
				.from(OrchestrationGraphEdges)
				.where(eq(OrchestrationGraphEdges.group_id, group_id))
				.orderBy(asc(OrchestrationGraphEdges.edge_id));
			const artifact_rows = yield* transaction
				.select()
				.from(OrchestrationArtifacts)
				.where(eq(OrchestrationArtifacts.group_id, group_id))
				.orderBy(
					asc(OrchestrationArtifacts.created_at),
					asc(OrchestrationArtifacts.artifact_id),
				);
			const assignments = yield* Effect.forEach(assignment_rows, (row) =>
				Effect.gen(function* () {
					const scope = yield* decode_json(
						AssignmentScope,
						row.scope_json,
						`Assignment ${row.assignment_id} scope`,
					);
					const workspace = yield* decode_json(
						AssignmentWorkspace,
						row.workspace_json,
						`Assignment ${row.assignment_id} workspace`,
					);
					const permission_policy = yield* decode_json(
						AssignmentPermissionPolicy,
						row.permission_policy_json,
						`Assignment ${row.assignment_id} permission policy`,
					);
					const heartbeat = row.heartbeat_json
						? yield* decode_json(
								AssignmentHeartbeat,
								row.heartbeat_json,
								`Assignment ${row.assignment_id} heartbeat`,
							)
						: undefined;

					return yield* Schema.decodeUnknownEffect(Assignment)({
						...(row.active_run_id ? { active_run_id: row.active_run_id } : {}),
						agent_id: row.agent_id,
						assignment_id: row.assignment_id,
						created_at: row.created_at,
						current_attempt: row.current_attempt,
						engine_id: row.engine_id,
						expected_result: row.expected_result,
						group_id: row.group_id,
						...(heartbeat ? { heartbeat } : {}),
						instructions: row.instructions,
						max_attempts: row.max_attempts,
						parent_node_id: row.parent_node_id,
						permission_policy,
						profile: row.profile,
						role: row.role,
						scope,
						state: row.state,
						summary_contract: row.summary_contract,
						updated_at: row.updated_at,
						workspace,
					}).pipe(
						Effect.mapError(
							() =>
								new AgentGraphInvalid({
									message: `Assignment ${row.assignment_id} is invalid`,
								}),
						),
					);
				}),
			);
			const agent_order = new Map([
				[group_row.coordinator_agent_id, 0],
				...assignment_rows.map(({ agent_id }, index) => [agent_id, index + 1] as const),
			]);
			const agent_instances = [...agent_rows].sort(
				(left, right) =>
					(agent_order.get(left.agent_id) ?? Number.MAX_SAFE_INTEGER) -
					(agent_order.get(right.agent_id) ?? Number.MAX_SAFE_INTEGER),
			);
			const agent_runs = yield* Effect.forEach(run_rows, (row) =>
				Effect.gen(function* () {
					const native_identity = row.native_identity_json
						? yield* decode_json(
								ProviderNativeIdentity,
								row.native_identity_json,
								`Agent run ${row.run_id} native identity`,
							)
						: undefined;
					const raw_origin = row.raw_origin_json
						? yield* decode_json(
								RawOrigin,
								row.raw_origin_json,
								`Agent run ${row.run_id} raw origin`,
							)
						: undefined;

					return yield* Schema.decodeUnknownEffect(AgentRun)({
						agent_id: row.agent_id,
						assignment_id: row.assignment_id,
						attempt: row.attempt,
						...(row.completed_at ? { completed_at: row.completed_at } : {}),
						created_at: row.created_at,
						engine_id: row.engine_id,
						execution_origin: row.execution_origin as
							| "artisan_dispatched"
							| "provider_observed",
						group_id: row.group_id,
						last_observation_sequence: row.last_observation_sequence,
						...(native_identity ? { native_identity } : {}),
						...(row.native_thread_id ? { native_thread_id: row.native_thread_id } : {}),
						profile: row.profile,
						...(raw_origin ? { raw_origin } : {}),
						run_id: row.run_id,
						state: row.state,
						updated_at: row.updated_at,
					}).pipe(
						Effect.mapError(
							() =>
								new AgentGraphInvalid({
									message: `Agent run ${row.run_id} is invalid`,
								}),
						),
					);
				}),
			);
			const joins = yield* Effect.forEach(join_rows, (row) =>
				decode_json(
					Schema.NonEmptyArray(Schema.String),
					row.upstream_assignment_ids_json,
					`Join ${row.join_id} upstream assignments`,
				).pipe(
					Effect.flatMap((upstream_assignment_ids) =>
						Schema.decodeUnknownEffect(Join)({
							created_at: row.created_at,
							...(row.downstream_assignment_id
								? { downstream_assignment_id: row.downstream_assignment_id }
								: {}),
							group_id: row.group_id,
							join_id: row.join_id,
							...(row.selected_assignment_id
								? { selected_assignment_id: row.selected_assignment_id }
								: {}),
							state: row.state,
							strategy: row.strategy,
							updated_at: row.updated_at,
							upstream_assignment_ids,
						}).pipe(
							Effect.mapError(
								() =>
									new AgentGraphInvalid({
										message: `Join ${row.join_id} is invalid`,
									}),
							),
						),
					),
				),
			);
			const artifacts = yield* Effect.forEach(artifact_rows, (row) =>
				Effect.gen(function* () {
					const raw_origin = row.raw_origin_json
						? yield* decode_json(
								RawOrigin,
								row.raw_origin_json,
								`Artifact ${row.artifact_id} raw origin`,
							)
						: undefined;

					return yield* Schema.decodeUnknownEffect(Artifact)({
						artifact_id: row.artifact_id,
						assignment_id: row.assignment_id,
						...(row.content ? { content: row.content } : {}),
						created_at: row.created_at,
						group_id: row.group_id,
						kind: row.kind,
						label: row.label,
						...(raw_origin ? { raw_origin } : {}),
						run_id: row.run_id,
						...(row.uri ? { uri: row.uri } : {}),
					}).pipe(
						Effect.mapError(
							() =>
								new AgentGraphInvalid({
									message: `Artifact ${row.artifact_id} is invalid`,
								}),
						),
					);
				}),
			);
			const group = yield* Schema.decodeUnknownEffect(OrchestrationGroup)({
				coordinator_agent_id: group_row.coordinator_agent_id,
				created_at: group_row.created_at,
				group_id: group_row.group_id,
				max_concurrency: group_row.max_concurrency,
				state: group_row.state,
				thread_id: group_row.thread_id,
				updated_at: group_row.updated_at,
				version: group_row.version,
			}).pipe(
				Effect.mapError(
					() => new AgentGraphInvalid({ message: `Group ${group_id} is invalid` }),
				),
			);
			const edges = yield* Effect.forEach(edge_rows, (row) =>
				Schema.decodeUnknownEffect(GraphEdge)({
					edge_id: row.edge_id,
					from_node_id: row.from_node_id,
					group_id: row.group_id,
					kind: row.kind,
					to_node_id: row.to_node_id,
				}).pipe(
					Effect.mapError(
						() =>
							new AgentGraphInvalid({
								message: `Graph edge ${row.edge_id} is invalid`,
							}),
					),
				),
			);

			return yield* Schema.decodeUnknownEffect(OrchestrationGraph)({
				agent_instances,
				agent_runs,
				artifacts,
				assignments,
				edges,
				group,
				joins,
				journal_sequence: group_row.journal_sequence,
			}).pipe(
				Effect.mapError(
					() =>
						new AgentGraphInvalid({
							message: `Graph ${group_id} projection is invalid`,
						}),
				),
			);
		});

	return { build_graph, decode_event_row, decode_json };
}
