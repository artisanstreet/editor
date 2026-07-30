import { and, asc, desc, eq, notExists } from "drizzle-orm";
import { Effect } from "effect";

import {
	AssignmentPermissionPolicy,
	AssignmentScope,
	AssignmentWorkspace,
	type OrchestrationGraph,
	type OrchestrationGroupListSnapshot,
	type OrchestrationGroupSummary,
} from "@artisan/protocol";

import {
	AgentRuns,
	Assignments,
	JournalEvents,
	OrchestrationGroups,
	ThreadErasureClaims,
} from "../../persistence/tables";
import {
	AgentGraphNotFound,
	normalize_graph_error,
	type AgentGraphError,
	type PendingAgentRun,
} from "../agent-graph-model";
import type { GraphContext, GraphTransaction } from "./graph-context";
import type { PersistedGraphCodecs } from "./persisted-graph-codecs";

export interface GraphQuery {
	readonly get_graph: (group_id: string) => Effect.Effect<OrchestrationGraph, AgentGraphError>;
	readonly list_groups: (
		thread_id: string,
		include_terminal: boolean,
	) => Effect.Effect<ReadonlyArray<OrchestrationGroupSummary>, AgentGraphError>;
	readonly list_groups_snapshot: (
		thread_id: string,
		include_terminal: boolean,
	) => Effect.Effect<OrchestrationGroupListSnapshot, AgentGraphError>;
	readonly get_pending_runs: () => Effect.Effect<ReadonlyArray<PendingAgentRun>, AgentGraphError>;
	readonly read_owned_assignment: (
		transaction: GraphTransaction,
		group_id: string,
		assignment_id: string,
		thread_id: string,
	) => Effect.Effect<
		{
			readonly assignment: typeof Assignments.$inferSelect;
			readonly group: typeof OrchestrationGroups.$inferSelect;
		},
		unknown,
		never
	>;
	readonly read_owned_group: (
		transaction: GraphTransaction,
		group_id: string,
		thread_id: string,
	) => Effect.Effect<typeof OrchestrationGroups.$inferSelect, unknown>;
}

/** Owns deterministic graph reads and ownership-scoped row lookup. */
export function make_graph_query(context: GraphContext, codecs: PersistedGraphCodecs): GraphQuery {
	const { database } = context;

	const read_owned_group = (transaction: GraphTransaction, group_id: string, thread_id: string) =>
		transaction
			.select()
			.from(OrchestrationGroups)
			.where(
				and(
					eq(OrchestrationGroups.group_id, group_id),
					eq(OrchestrationGroups.thread_id, thread_id),
				),
			)
			.limit(1)
			.pipe(
				Effect.flatMap(([group]) =>
					group
						? Effect.succeed(group)
						: Effect.fail(
								new AgentGraphNotFound({
									id: group_id,
									resource: "orchestration_group",
								}),
							),
				),
			);

	const read_owned_assignment = (
		transaction: GraphTransaction,
		group_id: string,
		assignment_id: string,
		thread_id: string,
	) =>
		Effect.gen(function* () {
			const group = yield* read_owned_group(transaction, group_id, thread_id);
			const [assignment] = yield* transaction
				.select()
				.from(Assignments)
				.where(
					and(
						eq(Assignments.assignment_id, assignment_id),
						eq(Assignments.group_id, group_id),
					),
				)
				.limit(1);

			if (!assignment) {
				return yield* new AgentGraphNotFound({
					id: assignment_id,
					resource: "assignment",
				});
			}

			return { assignment, group };
		});

	const get_graph = (group_id: string) =>
		codecs.build_graph(database.client, group_id).pipe(Effect.mapError(normalize_graph_error));

	const list_group_rows = (transaction: GraphTransaction, thread_id: string) =>
		transaction
			.select()
			.from(OrchestrationGroups)
			.where(eq(OrchestrationGroups.thread_id, thread_id))
			.orderBy(desc(OrchestrationGroups.updated_at), asc(OrchestrationGroups.group_id));

	const summarize_groups = (
		rows: ReadonlyArray<typeof OrchestrationGroups.$inferSelect>,
		include_terminal: boolean,
	) =>
		rows
			.filter(
				(row) =>
					include_terminal ||
					!["summarized", "stopped", "failed", "complete"].includes(row.state),
			)
			.map((row) => ({
				coordinator_agent_id: row.coordinator_agent_id,
				created_at: row.created_at,
				group_id: row.group_id,
				max_concurrency: row.max_concurrency,
				state: row.state as OrchestrationGroupSummary["state"],
				thread_id: row.thread_id,
				updated_at: row.updated_at,
				version: row.version,
			}));

	const list_groups = (thread_id: string, include_terminal: boolean) =>
		list_group_rows(database.client, thread_id).pipe(
			Effect.map((rows) => summarize_groups(rows, include_terminal)),
			Effect.mapError(normalize_graph_error),
		);

	const list_groups_snapshot = (thread_id: string, include_terminal: boolean) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const rows = yield* list_group_rows(transaction, thread_id);
					const [watermark] = yield* transaction
						.select({ journal_sequence: JournalEvents.sequence })
						.from(JournalEvents)
						.orderBy(desc(JournalEvents.sequence))
						.limit(1);
					return {
						groups: summarize_groups(rows, include_terminal),
						journal_sequence: watermark?.journal_sequence ?? 0,
					};
				}),
			)
			.pipe(Effect.mapError(normalize_graph_error));

	const get_pending_runs = () =>
		database.client
			.select({
				agent_id: AgentRuns.agent_id,
				assignment_id: AgentRuns.assignment_id,
				attempt: AgentRuns.attempt,
				engine_id: AgentRuns.engine_id,
				expected_result: Assignments.expected_result,
				group_id: AgentRuns.group_id,
				instructions: Assignments.instructions,
				max_concurrency: OrchestrationGroups.max_concurrency,
				permission_policy_json: Assignments.permission_policy_json,
				profile: AgentRuns.profile,
				run_id: AgentRuns.run_id,
				scope_json: Assignments.scope_json,
				summary_contract: Assignments.summary_contract,
				thread_id: OrchestrationGroups.thread_id,
				workspace_json: Assignments.workspace_json,
			})
			.from(AgentRuns)
			.innerJoin(Assignments, eq(AgentRuns.assignment_id, Assignments.assignment_id))
			.innerJoin(OrchestrationGroups, eq(AgentRuns.group_id, OrchestrationGroups.group_id))
			.where(
				and(
					eq(AgentRuns.dispatch_status, "queued"),
					notExists(
						database.client
							.select({ thread_id: ThreadErasureClaims.thread_id })
							.from(ThreadErasureClaims)
							.where(
								eq(ThreadErasureClaims.thread_id, OrchestrationGroups.thread_id),
							),
					),
				),
			)
			.orderBy(asc(AgentRuns.created_at), asc(AgentRuns.run_id))
			.pipe(
				Effect.flatMap((rows) =>
					Effect.forEach(rows, (row) =>
						Effect.gen(function* () {
							const scope = yield* codecs.decode_json(
								AssignmentScope,
								row.scope_json,
								`Assignment ${row.assignment_id} scope`,
							);
							const workspace = yield* codecs.decode_json(
								AssignmentWorkspace,
								row.workspace_json,
								`Assignment ${row.assignment_id} workspace`,
							);
							const permission_policy = yield* codecs.decode_json(
								AssignmentPermissionPolicy,
								row.permission_policy_json,
								`Assignment ${row.assignment_id} permission policy`,
							);

							return {
								agent_id: row.agent_id,
								assignment_id: row.assignment_id,
								attempt: row.attempt,
								engine_id: row.engine_id,
								expected_result: row.expected_result,
								group_id: row.group_id,
								instructions: row.instructions,
								max_concurrency: row.max_concurrency,
								permission_policy,
								profile: row.profile,
								run_id: row.run_id,
								scope,
								summary_contract: row.summary_contract,
								thread_id: row.thread_id,
								workspace,
							} satisfies PendingAgentRun;
						}),
					),
				),
				Effect.mapError(normalize_graph_error),
			);

	return {
		get_graph,
		get_pending_runs,
		list_groups,
		list_groups_snapshot,
		read_owned_assignment,
		read_owned_group,
	};
}
