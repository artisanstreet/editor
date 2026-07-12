import { sql } from "drizzle-orm";
import {
	blob,
	check,
	index,
	integer,
	primaryKey,
	sqliteTable,
	text,
	uniqueIndex,
} from "drizzle-orm/sqlite-core";

import { workspace_text_maximum_bytes } from "@artisan/protocol";

export const JournalCommands = sqliteTable(
	"journal_commands",
	{
		message_id: text("message_id").primaryKey(),
		schema_version: integer("schema_version").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id"),
		assigned_run_id: text("assigned_run_id"),
		agent_id: text("agent_id"),
		causation_id: text("causation_id"),
		origin: text("origin").notNull(),
		raw_origin_json: text("raw_origin_json"),
		sent_at: text("sent_at").notNull(),
		payload_type: text("payload_type").notNull(),
		payload_json: text("payload_json").notNull(),
		status: text("status").notNull(),
		accepted_at: text("accepted_at").notNull(),
	},
	(table) => [
		uniqueIndex("journal_commands_refinement_source_unique")
			.on(table.thread_id, table.causation_id)
			.where(
				sql`${table.origin} = 'backend' AND ${table.payload_type} = 'thread.metadata.refine'`,
			),
	],
);

export const EventStreams = sqliteTable("event_streams", {
	stream_id: text("stream_id").primaryKey(),
	last_sequence: integer("last_sequence").notNull(),
});

export const JournalEvents = sqliteTable(
	"journal_events",
	{
		sequence: integer("sequence").primaryKey({ autoIncrement: true }),
		stream_id: text("stream_id").notNull(),
		stream_sequence: integer("stream_sequence").notNull(),
		schema_version: integer("schema_version").notNull(),
		event_id: text("event_id").notNull(),
		correlation_id: text("correlation_id").notNull(),
		causation_id: text("causation_id").notNull(),
		origin: text("origin").notNull(),
		raw_origin_json: text("raw_origin_json"),
		event_type: text("event_type").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id"),
		agent_id: text("agent_id"),
		payload_json: text("payload_json").notNull(),
		occurred_at: text("occurred_at").notNull(),
	},
	(table) => [
		uniqueIndex("journal_events_event_id_unique").on(table.event_id),
		uniqueIndex("journal_events_stream_sequence_unique").on(
			table.stream_id,
			table.stream_sequence,
		),
		index("journal_events_correlation_id_index").on(table.correlation_id),
	],
);

export const Threads = sqliteTable(
	"threads",
	{
		thread_id: text("thread_id").primaryKey(),
		title: text("title").notNull(),
		title_source: text("title_source").notNull().default("initial"),
		title_locked: integer("title_locked", { mode: "boolean" }).notNull().default(false),
		live_status: text("live_status").notNull().default("Idle"),
		current_goal: text("current_goal"),
		rename_suggestion: text("rename_suggestion"),
		last_activity_at: text("last_activity_at").notNull().default("1970-01-01T00:00:00.000Z"),
		activity_version: integer("activity_version").notNull().default(0),
		metadata_version: integer("metadata_version").notNull().default(0),
		affinity_version: integer("affinity_version").notNull().default(0),
		primary_project_id: text("primary_project_id"),
		primary_project_json: text("primary_project_json"),
		linked_projects_json: text("linked_projects_json").notNull().default("[]"),
		project_locked: integer("project_locked", { mode: "boolean" }).notNull().default(false),
		project_affinity_scores_json: text("project_affinity_scores_json").notNull().default("[]"),
		rehome_suggestion_json: text("rehome_suggestion_json"),
		pinned: integer("pinned", { mode: "boolean" }).notNull().default(false),
		archived_at: text("archived_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("threads_retention_candidates_index").on(table.pinned, table.last_activity_at),
		index("threads_primary_project_index").on(table.primary_project_id),
	],
);

/** Stores idempotent, content-free evidence used to calculate project affinity. */
export const ThreadProjectAffinityEvidence = sqliteTable(
	"thread_project_affinity_evidence",
	{
		basis_affinity_version: integer("basis_affinity_version").notNull(),
		evidence_id: text("evidence_id").primaryKey(),
		kind: text("kind").notNull(),
		observed_at: text("observed_at").notNull(),
		project_id: text("project_id").notNull(),
		project_json: text("project_json").notNull(),
		source_event_id: text("source_event_id").notNull(),
		source_journal_sequence: integer("source_journal_sequence").notNull(),
		thread_id: text("thread_id").notNull(),
	},
	(table) => [
		index("thread_project_affinity_evidence_thread_index").on(table.thread_id),
		uniqueIndex("thread_project_affinity_evidence_source_unique").on(
			table.thread_id,
			table.source_event_id,
			table.kind,
			table.project_id,
		),
	],
);

export const ThreadRetentionPolicies = sqliteTable("thread_retention_policies", {
	policy_id: integer("policy_id").primaryKey(),
	enabled: integer("enabled", { mode: "boolean" }).notNull(),
	inactivity_days: integer("inactivity_days").notNull(),
	updated_at: text("updated_at").notNull(),
});

/** Stores only content-free metadata for the one canonical global guidance file. */
export const GlobalGuidanceCanonical = sqliteTable("global_guidance_canonical", {
	canonical_id: integer("canonical_id").primaryKey(),
	content_hash: text("content_hash"),
	byte_count: integer("byte_count"),
	status: text("status").notNull(),
	selected_provider: text("selected_provider"),
	updated_at: text("updated_at").notNull(),
});

/** Stores metadata-only reconciliation state for each native provider mirror. */
export const GlobalGuidanceProviderSync = sqliteTable("global_guidance_provider_sync", {
	provider: text("provider").primaryKey(),
	status: text("status").notNull(),
	path: text("path"),
	modified_at: text("modified_at"),
	observed_hash: text("observed_hash"),
	observed_byte_count: integer("observed_byte_count"),
	applied_hash: text("applied_hash"),
	applied_byte_count: integer("applied_byte_count"),
	ignored_drift_hash: text("ignored_drift_hash"),
	backup_path: text("backup_path"),
	last_error_code: text("last_error_code"),
	updated_at: text("updated_at").notNull(),
});

/** Stores the one curated provider-neutral model behaviour setting. */
export const ModelBehaviourSettings = sqliteTable("model_behaviour_settings", {
	setting_id: text("setting_id").primaryKey(),
	value_json: text("value_json").notNull(),
	version: integer("version").notNull(),
	updated_at: text("updated_at").notNull(),
});

/** Stores content-free reconciliation metadata for each provider setting mapping. */
export const ModelBehaviourProviderStates = sqliteTable(
	"model_behaviour_provider_states",
	{
		provider_id: text("provider_id").notNull(),
		setting_id: text("setting_id").notNull(),
		status: text("status").notNull(),
		native_key: text("native_key"),
		target_path: text("target_path"),
		observed_hash: text("observed_hash"),
		applied_hash: text("applied_hash"),
		ignored_drift_hash: text("ignored_drift_hash"),
		backup_path: text("backup_path"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.provider_id, table.setting_id] })],
);

/** Stores idempotent, content-free workspace change operation lifecycles. */
export const WorkspaceChangeOperations = sqliteTable(
	"workspace_change_operations",
	{
		message_id: text("message_id").primaryKey(),
		action: text("action").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		change_id: text("change_id").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id"),
		agent_id: text("agent_id"),
		raw_origin_json: text("raw_origin_json"),
		workspace_id: text("workspace_id"),
		path: text("path"),
		expected_identity_json: text("expected_identity_json"),
		result_identity_json: text("result_identity_json"),
		lifecycle: text("lifecycle").notNull(),
		evidence_recorded: integer("evidence_recorded", { mode: "boolean" })
			.notNull()
			.default(false),
		journal_sequence: integer("journal_sequence"),
		sent_at: text("sent_at").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_change_operations_change_action_unique").on(
			table.change_id,
			table.action,
		),
		index("workspace_change_operations_change_id_index").on(table.change_id),
	],
);

/** Stores durable, source-free workspace change projections. */
export const WorkspaceChanges = sqliteTable(
	"workspace_changes",
	{
		change_id: text("change_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		path: text("path").notNull(),
		before_identity_json: text("before_identity_json").notNull(),
		after_identity_json: text("after_identity_json").notNull(),
		run_id: text("run_id").notNull(),
		agent_id: text("agent_id").notNull(),
		raw_origin_json: text("raw_origin_json"),
		review_state: text("review_state").notNull(),
		rollback_state: text("rollback_state").notNull(),
		reviewed_at: text("reviewed_at"),
		rolled_back_at: text("rolled_back_at"),
		version: integer("version").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_changes_source_command_unique").on(table.source_command_id),
		index("workspace_changes_thread_id_index").on(table.thread_id),
		index("workspace_changes_thread_workspace_index").on(table.thread_id, table.workspace_id),
	],
);

/** Stores opaque rollback bytes outside all journal and workspace-change projections. */
export const WorkspaceChangeSnapshots = sqliteTable(
	"workspace_change_snapshots",
	{
		change_id: text("change_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		state: text("state").notNull(),
		content: blob("content", { mode: "buffer" }),
		byte_count: integer("byte_count"),
		content_hash: text("content_hash"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("workspace_change_snapshots_thread_id_index").on(table.thread_id),
		check(
			"workspace_change_snapshots_state_check",
			sql`${table.state} IN ('available', 'consumed')`,
		),
		check(
			"workspace_change_snapshots_content_check",
			sql`
				(
					${table.state} = 'available'
					AND ${table.content} IS NOT NULL
					AND ${table.byte_count} IS NOT NULL
					AND ${table.content_hash} IS NOT NULL
					AND length(${table.content}) = ${table.byte_count}
					AND ${table.byte_count} BETWEEN 0 AND ${sql.raw(String(workspace_text_maximum_bytes))}
					AND length(${table.content_hash}) = 64
					AND ${table.content_hash} NOT GLOB '*[^0-9a-f]*'
				)
				OR (
					${table.state} = 'consumed'
					AND ${table.content} IS NULL
					AND ${table.byte_count} IS NULL
					AND ${table.content_hash} IS NULL
				)
			`,
		),
	],
);

export const ThreadErasureClaims = sqliteTable("thread_erasure_claims", {
	thread_id: text("thread_id").primaryKey(),
	claimed_at: text("claimed_at").notNull(),
});

export const ThreadTombstones = sqliteTable("thread_tombstones", {
	thread_id: text("thread_id").primaryKey(),
	deleted_at: text("deleted_at").notNull(),
});

export const OrchestrationCoordinators = sqliteTable("orchestration_coordinators", {
	thread_id: text("thread_id").primaryKey(),
	agent_id: text("agent_id").notNull(),
	role: text("role").notNull(),
	display_name: text("display_name").notNull(),
	engine_id: text("engine_id").notNull(),
	active_run_id: text("active_run_id"),
	native_thread_id: text("native_thread_id"),
	native_resume_json: text("native_resume_json"),
	created_at: text("created_at").notNull(),
	updated_at: text("updated_at").notNull(),
});

export const OrchestrationRuns = sqliteTable(
	"orchestration_runs",
	{
		run_id: text("run_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		agent_id: text("agent_id").notNull(),
		engine_id: text("engine_id").notNull(),
		working_directory: text("working_directory").notNull(),
		status: text("status").notNull(),
		native_thread_id: text("native_thread_id"),
		native_resume_json: text("native_resume_json"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("orchestration_runs_thread_id_index").on(table.thread_id),
		index("orchestration_runs_status_index").on(table.status),
	],
);

export const OrchestrationMessages = sqliteTable("orchestration_messages", {
	message_id: text("message_id").primaryKey(),
	command_id: text("command_id").notNull(),
	thread_id: text("thread_id").notNull(),
	run_id: text("run_id"),
	agent_id: text("agent_id").notNull(),
	text: text("text").notNull(),
	delivery: text("delivery").notNull(),
	created_at: text("created_at").notNull(),
});

export const OrchestrationInteractions = sqliteTable(
	"orchestration_interactions",
	{
		interaction_id: text("interaction_id").notNull(),
		run_id: text("run_id").notNull(),
		kind: text("kind").notNull(),
		description: text("description").notNull(),
		state: text("state").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.run_id, table.kind, table.interaction_id] })],
);

export const OrchestrationRawObservations = sqliteTable(
	"orchestration_raw_observations",
	{
		observation_id: text("observation_id").primaryKey(),
		run_id: text("run_id").notNull(),
		engine_id: text("engine_id").notNull(),
		sequence: integer("sequence").notNull(),
		native_id: text("native_id"),
		native_method: text("native_method"),
		transport: text("transport").notNull(),
		protocol_version: text("protocol_version"),
		frame_json: text("frame_json").notNull(),
		raw_frame_base64: text("raw_frame_base64"),
	},
	(table) => [
		index("orchestration_raw_observations_run_sequence_index").on(table.run_id, table.sequence),
	],
);

export const OrchestrationOutbox = sqliteTable(
	"orchestration_outbox",
	{
		command_id: text("command_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		agent_id: text("agent_id").notNull(),
		kind: text("kind").notNull(),
		payload_json: text("payload_json").notNull(),
		status: text("status").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("orchestration_outbox_status_index").on(table.status),
		index("orchestration_outbox_run_id_index").on(table.run_id),
	],
);

export const OrchestrationGroups = sqliteTable(
	"orchestration_groups",
	{
		group_id: text("group_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		coordinator_agent_id: text("coordinator_agent_id").notNull(),
		state: text("state").notNull(),
		max_concurrency: integer("max_concurrency").notNull(),
		version: integer("version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("orchestration_groups_thread_id_index").on(table.thread_id),
		index("orchestration_groups_state_index").on(table.state),
	],
);

export const AgentInstances = sqliteTable(
	"agent_instances",
	{
		agent_id: text("agent_id").primaryKey(),
		group_id: text("group_id").notNull(),
		display_name: text("display_name").notNull(),
		role: text("role").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("agent_instances_group_display_name_unique").on(
			table.group_id,
			table.display_name,
		),
		index("agent_instances_group_id_index").on(table.group_id),
	],
);

export const Assignments = sqliteTable(
	"assignments",
	{
		assignment_id: text("assignment_id").primaryKey(),
		group_id: text("group_id").notNull(),
		agent_id: text("agent_id").notNull(),
		role: text("role").notNull(),
		scope_json: text("scope_json").notNull(),
		engine_id: text("engine_id").notNull(),
		profile: text("profile").notNull(),
		workspace_json: text("workspace_json").notNull(),
		permission_policy_json: text("permission_policy_json").notNull(),
		summary_contract: text("summary_contract").notNull(),
		parent_node_id: text("parent_node_id").notNull(),
		expected_result: text("expected_result").notNull(),
		instructions: text("instructions").notNull(),
		state: text("state").notNull(),
		current_attempt: integer("current_attempt").notNull(),
		max_attempts: integer("max_attempts").notNull(),
		active_run_id: text("active_run_id"),
		heartbeat_json: text("heartbeat_json"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("assignments_group_id_index").on(table.group_id),
		index("assignments_state_index").on(table.state),
		index("assignments_active_run_id_index").on(table.active_run_id),
	],
);

export const AgentRuns = sqliteTable(
	"agent_runs",
	{
		run_id: text("run_id").primaryKey(),
		group_id: text("group_id").notNull(),
		assignment_id: text("assignment_id").notNull(),
		agent_id: text("agent_id").notNull(),
		attempt: integer("attempt").notNull(),
		engine_id: text("engine_id").notNull(),
		profile: text("profile").notNull(),
		state: text("state").notNull(),
		dispatch_status: text("dispatch_status").notNull(),
		owner_instance_id: text("owner_instance_id"),
		native_thread_id: text("native_thread_id"),
		native_resume_json: text("native_resume_json"),
		native_identity_json: text("native_identity_json"),
		raw_origin_json: text("raw_origin_json"),
		last_observation_sequence: integer("last_observation_sequence").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		completed_at: text("completed_at"),
	},
	(table) => [
		uniqueIndex("agent_runs_assignment_attempt_unique").on(table.assignment_id, table.attempt),
		index("agent_runs_group_id_index").on(table.group_id),
		index("agent_runs_dispatch_status_index").on(table.dispatch_status),
		index("agent_runs_assignment_id_index").on(table.assignment_id),
	],
);

export const OrchestrationJoins = sqliteTable(
	"orchestration_joins",
	{
		join_id: text("join_id").primaryKey(),
		group_id: text("group_id").notNull(),
		strategy: text("strategy").notNull(),
		state: text("state").notNull(),
		upstream_assignment_ids_json: text("upstream_assignment_ids_json").notNull(),
		downstream_assignment_id: text("downstream_assignment_id"),
		selected_assignment_id: text("selected_assignment_id"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [index("orchestration_joins_group_id_index").on(table.group_id)],
);

export const OrchestrationGraphEdges = sqliteTable(
	"orchestration_graph_edges",
	{
		edge_id: text("edge_id").primaryKey(),
		group_id: text("group_id").notNull(),
		from_node_id: text("from_node_id").notNull(),
		to_node_id: text("to_node_id").notNull(),
		kind: text("kind").notNull(),
		dispatch_dependency: integer("dispatch_dependency").notNull(),
	},
	(table) => [index("orchestration_graph_edges_group_id_index").on(table.group_id)],
);

export const OrchestrationArtifacts = sqliteTable(
	"orchestration_artifacts",
	{
		artifact_id: text("artifact_id").primaryKey(),
		group_id: text("group_id").notNull(),
		assignment_id: text("assignment_id").notNull(),
		run_id: text("run_id").notNull(),
		kind: text("kind").notNull(),
		label: text("label").notNull(),
		content: text("content"),
		uri: text("uri"),
		raw_origin_json: text("raw_origin_json"),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		index("orchestration_artifacts_group_id_index").on(table.group_id),
		index("orchestration_artifacts_assignment_id_index").on(table.assignment_id),
	],
);

export const OrchestrationGraphCommands = sqliteTable(
	"orchestration_graph_commands",
	{
		message_id: text("message_id").primaryKey(),
		group_id: text("group_id").notNull(),
		assignment_id: text("assignment_id"),
		run_id: text("run_id"),
		action: text("action").notNull(),
		status: text("status").notNull(),
		outcome: text("outcome"),
		journal_sequence: integer("journal_sequence"),
		failure: text("failure"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("orchestration_graph_commands_group_id_index").on(table.group_id),
		index("orchestration_graph_commands_status_index").on(table.status),
	],
);

export const TerminalSessions = sqliteTable(
	"terminal_sessions",
	{
		terminal_id: text("terminal_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		working_directory: text("working_directory").notNull(),
		executable: text("executable").notNull(),
		args_json: text("args_json").notNull(),
		env_json: text("env_json"),
		cols: integer("cols").notNull(),
		generation: integer("generation").notNull(),
		rows: integer("rows").notNull(),
		pid: integer("pid"),
		owner_instance_id: text("owner_instance_id").notNull(),
		state: text("state").notNull(),
		exit_code: integer("exit_code"),
		exit_signal: integer("exit_signal"),
		exit_reason: text("exit_reason"),
		failure: text("failure"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		closed_at: text("closed_at"),
	},
	(table) => [
		index("terminal_sessions_thread_workspace_index").on(table.thread_id, table.workspace_id),
		index("terminal_sessions_state_index").on(table.state),
	],
);

export const TerminalCommands = sqliteTable("terminal_commands", {
	message_id: text("message_id").primaryKey(),
	terminal_id: text("terminal_id").notNull(),
	generation: integer("generation").notNull(),
	claimed_session_json: text("claimed_session_json").notNull(),
	payload_json: text("payload_json").notNull(),
	status: text("status").notNull(),
	journal_sequence: integer("journal_sequence"),
	failure: text("failure"),
	created_at: text("created_at").notNull(),
	updated_at: text("updated_at").notNull(),
});
