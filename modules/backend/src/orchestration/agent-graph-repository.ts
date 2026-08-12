import { Context, Effect, Layer } from "effect";

import { Database } from "../persistence/database";
import { JournalNotifier } from "../persistence/journal-notifier";
import { NativeSubagentBindings } from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";
import { AgentNameCatalog } from "./agent-name-catalog";
import { make_assignment_commands } from "./internal/assignment-commands";
import { make_control_commands } from "./internal/control-commands";
import { make_dependency_evaluation } from "./internal/dependency-evaluation";
import { make_graph_advancement } from "./internal/graph-advancement";
import { make_graph_ledger } from "./internal/graph-ledger";
import { make_graph_query } from "./internal/graph-query";
import { make_graph_topology } from "./internal/graph-topology";
import { make_group_start } from "./internal/group-start";
import { make_join_evaluation } from "./internal/join-evaluation";
import { make_native_subagents } from "./internal/native-subagents";
import { make_persisted_graph_codecs } from "./internal/persisted-graph-codecs";
import { make_run_lifecycle } from "./internal/run-lifecycle";
import { make_run_transitions } from "./internal/run-transitions";
import type { AgentGraphRepositoryShape } from "./agent-graph-model";

export {
	AgentGraphCommandConflict,
	AgentGraphFailure,
	AgentGraphInvalid,
	AgentGraphNotFound,
	normalize_graph_error,
	type AcceptedAgentGraphCommand,
	type AgentGraphCommand,
	type AgentGraphControlAction,
	type AgentGraphControlClaim,
	type AgentGraphControlOutcome,
	type AgentGraphError,
	type AgentRunActivation,
	type PendingAgentRun,
} from "./agent-graph-model";

/** Exposes the small durable capability surface for one multi-agent graph. */
export class AgentGraphRepository extends Context.Service<
	AgentGraphRepository,
	AgentGraphRepositoryShape
>()("Artisan/AgentGraphRepository") {}

export const AgentGraphRepositoryLive = Layer.effect(
	AgentGraphRepository,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;
		const agent_name_catalog = yield* AgentNameCatalog;
		const context = { agent_name_catalog, database, metadata, notifier };
		const codecs = make_persisted_graph_codecs(context);
		const ledger = make_graph_ledger(context, codecs);
		const query = make_graph_query(context, codecs);
		const topology = make_graph_topology(context);
		const dependencies = make_dependency_evaluation(context, ledger);
		const joins = make_join_evaluation(context, codecs, ledger);
		const advancement = make_graph_advancement(dependencies, joins);
		const transitions = make_run_transitions(context, advancement, ledger);
		const group_start = make_group_start(context, ledger, topology);
		const assignment_commands = make_assignment_commands(
			context,
			codecs,
			dependencies,
			ledger,
			query,
		);
		const control_commands = make_control_commands(context, ledger, query);
		const run_lifecycle = make_run_lifecycle(context, ledger, transitions);
		const native_subagents = make_native_subagents(context, ledger);

		return {
			ActivateRun: run_lifecycle.activate_run,
			ClaimControl: control_commands.claim_control,
			ClaimRun: run_lifecycle.claim_run,
			CompleteControl: control_commands.complete_control,
			FailRunStart: run_lifecycle.fail_run_start,
			FinalizeControl: control_commands.finalize_control,
			GetGraph: query.get_graph,
			ListGroups: query.list_groups,
			ListGroupsSnapshot: query.list_groups_snapshot,
			GetPendingRuns: query.get_pending_runs,
			ReadCommandEvents: control_commands.read_command_events,
			RecordClosed: run_lifecycle.record_closed,
			RecordHeartbeat: assignment_commands.record_heartbeat,
			RecordObservation: run_lifecycle.record_observation,
			RecordObservedSubagent: native_subagents.Record,
			ReconcileObservedRoot: native_subagents.ReconcileRoot,
			RecoverObservedSubagents: native_subagents.Recover,
			ReconcileObservedSubagentsExcept: (provisional_root_run_ids) =>
				database.client
					.selectDistinct({ root_run_id: NativeSubagentBindings.root_run_id })
					.from(NativeSubagentBindings)
					.pipe(
						Effect.flatMap((roots) =>
							Effect.forEach(
								roots.filter(
									(root) => !provisional_root_run_ids.has(root.root_run_id),
								),
								(root) => native_subagents.ReconcileRoot(root.root_run_id),
								{ discard: true },
							),
						),
					),
			Recover: run_lifecycle.recover,
			RenameAgent: assignment_commands.rename_agent,
			RetryAssignment: assignment_commands.retry_assignment,
			StartGroup: group_start.start_group,
		};
	}),
);
