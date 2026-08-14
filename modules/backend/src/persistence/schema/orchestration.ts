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
	auto_steer_follow_ups: integer("auto_steer_follow_ups", { mode: "boolean" })
		.notNull()
		.default(true),
	policy_model: text("policy_model"),
	policy_context_window: text("policy_context_window"),
	policy_reasoning_effort: text("policy_reasoning_effort").notNull().default("medium"),
	policy_permission: text("policy_permission").notNull().default("supervised"),
	policy_permission_mode: text("policy_permission_mode").notNull().default("on_request"),
	policy_sandbox_mode: text("policy_sandbox_mode").notNull().default("workspace_write"),
	policy_service_tier: text("policy_service_tier").notNull().default("standard"),
	policy_workflow_mode: text("policy_workflow_mode").notNull().default("build"),
	policy_web_search_enabled: integer("policy_web_search_enabled", { mode: "boolean" })
		.notNull()
		.default(false),
	policy_strict_clarification: integer("policy_strict_clarification", { mode: "boolean" })
		.notNull()
		.default(false),
});

/** Durable pre-execution intake state; a pending row deliberately has no run. */
export const OrchestrationIntake = sqliteTable(
	"orchestration_intake",
	{
		message_id: text("message_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		engine_id: text("engine_id").notNull(),
		working_directory: text("working_directory").notNull(),
		text: text("text").notNull(),
		mentioned_projects_json: text("mentioned_projects_json"),
		raw_origin_json: text("raw_origin_json"),
		risk: text("risk").notNull(),
		state: text("state").notNull(),
		question_id: text("question_id"),
		question: text("question"),
		assumptions_json: text("assumptions_json").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("orchestration_intake_question_id_unique").on(table.question_id),
		index("orchestration_intake_thread_state_index").on(table.thread_id, table.state),
		check(
			"orchestration_intake_risk_check",
			sql`${table.risk} IN ('low', 'material', 'high', 'underspecified')`,
		),
		check("orchestration_intake_state_check", sql`${table.state} IN ('pending', 'resolved')`),
		check(
			"orchestration_intake_question_shape_check",
			sql`(${table.state} = 'pending' AND ${table.question_id} IS NOT NULL AND ${table.question} IS NOT NULL) OR (${table.state} = 'resolved')`,
		),
	],
);

export const OrchestrationRuns = sqliteTable(
	"orchestration_runs",
	{
		run_id: text("run_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		agent_id: text("agent_id").notNull(),
		engine_id: text("engine_id").notNull(),
		/** Catalog model resolved at dispatch; null for runs stamped before collection began. */
		model_id: text("model_id"),
		working_directory: text("working_directory").notNull(),
		status: text("status").notNull(),
		/** Durable monotonic receipt; full provider frames are never retained. */
		last_observation_sequence: integer("last_observation_sequence").notNull().default(-1),
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

/** One durable recovery decision for an exact provider usage-limit failure. */
export const UsageInterruptions = sqliteTable(
	"usage_interruptions",
	{
		affected_model_id: text("affected_model_id"),
		alternatives_json: text("alternatives_json").notNull().default("[]"),
		auto_continue: integer("auto_continue", { mode: "boolean" }).notNull(),
		cancelled_at: text("cancelled_at"),
		continuation_command_id: text("continuation_command_id"),
		continued_at: text("continued_at"),
		created_at: text("created_at").notNull(),
		evidence_refreshed_at: text("evidence_refreshed_at"),
		failed_at: text("failed_at"),
		interruption_id: text("interruption_id").primaryKey(),
		limit_id: text("limit_id"),
		limit_label: text("limit_label"),
		limit_scope: text("limit_scope").notNull(),
		provider_code: text("provider_code"),
		resets_at: text("resets_at"),
		resume_not_before: text("resume_not_before"),
		revision: integer("revision").notNull().default(0),
		source_agent_id: text("source_agent_id").notNull(),
		source_engine_id: text("source_engine_id").notNull(),
		source_model_id: text("source_model_id"),
		source_run_id: text("source_run_id").notNull(),
		state: text("state").notNull(),
		target_engine_id: text("target_engine_id"),
		target_model_id: text("target_model_id"),
		target_run_id: text("target_run_id"),
		thread_id: text("thread_id").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("usage_interruptions_source_run_unique").on(table.source_run_id),
		index("usage_interruptions_due_index").on(table.state, table.resume_not_before),
		index("usage_interruptions_thread_index").on(table.thread_id),
		check(
			"usage_interruptions_limit_scope_check",
			sql`${table.limit_scope} IN ('shared', 'model', 'unknown')`,
		),
		check(
			"usage_interruptions_state_check",
			sql`${table.state} IN ('scheduled', 'awaiting_decision', 'launching', 'continued', 'cancelled', 'failed')`,
		),
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

/** Binary user image evidence. Journal, events, and conversation projections retain references only. */
export const MessageImageAttachments = sqliteTable(
	"message_image_attachments",
	{
		attachment_id: text("attachment_id").primaryKey(),
		message_id: text("message_id").notNull(),
		name: text("name").notNull(),
		media_type: text("media_type").notNull(),
		size_bytes: integer("size_bytes").notNull(),
		content: blob("content", { mode: "buffer" }).notNull(),
		position: integer("position").notNull(),
	},
	(table) => [
		index("message_image_attachments_message_index").on(table.message_id, table.position),
		check(
			"message_image_attachments_media_type_check",
			sql`${table.media_type} IN ('image/gif', 'image/jpeg', 'image/png', 'image/webp')`,
		),
	],
);

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

/**
 * Durable handoff from canonical engine observation persistence to the
 * provider-native orchestration projection. The observation and this inbox row
 * commit together; graph adoption and `processed_at` commit together later.
 */
export const NativeSubagentObservationInbox = sqliteTable(
	"native_subagent_observation_inbox",
	{
		observation_id: text("observation_id").primaryKey(),
		root_run_id: text("root_run_id").notNull(),
		engine_id: text("engine_id").notNull(),
		agent_native_thread_id: text("agent_native_thread_id").notNull(),
		parent_native_thread_id: text("parent_native_thread_id").notNull(),
		state: text("state").notNull(),
		sequence: integer("sequence").notNull(),
		activity: text("activity"),
		agent_path: text("agent_path"),
		turn_id: text("turn_id"),
		native_id: text("native_id"),
		created_at: text("created_at").notNull(),
		processed_at: text("processed_at"),
	},
	(table) => [
		index("native_subagent_observation_inbox_pending_index").on(
			table.processed_at,
			table.root_run_id,
			table.sequence,
		),
	],
);

/**
 * Canonical child-thread observations wait here until the matching native
 * lifecycle binding exists. Keeping their own inbox means a provider can emit
 * a child's final message before (or after) its lifecycle frame without ever
 * projecting it as root-run content.
 */
export const NativeSubagentTranscriptInbox = sqliteTable(
	"native_subagent_transcript_inbox",
	{
		observation_id: text("observation_id").primaryKey(),
		root_run_id: text("root_run_id").notNull(),
		engine_id: text("engine_id").notNull(),
		agent_native_thread_id: text("agent_native_thread_id").notNull(),
		parent_native_thread_id: text("parent_native_thread_id").notNull(),
		sequence: integer("sequence").notNull(),
		content_json: text("content_json").notNull(),
		created_at: text("created_at").notNull(),
		processed_at: text("processed_at"),
	},
	(table) => [
		index("native_subagent_transcript_inbox_pending_index").on(
			table.processed_at,
			table.root_run_id,
			table.agent_native_thread_id,
			table.sequence,
		),
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
		execution_origin: text("execution_origin").notNull().default("artisan_dispatched"),
		/** Catalog model resolved at dispatch; null until the run activates. */
		model_id: text("model_id"),
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

/** Idempotent provider-native child identity bindings for one ordinary root run. */
export const NativeSubagentBindings = sqliteTable(
	"native_subagent_bindings",
	{
		binding_id: text("binding_id").primaryKey(),
		engine_id: text("engine_id").notNull(),
		root_run_id: text("root_run_id").notNull(),
		group_id: text("group_id").notNull(),
		parent_native_thread_id: text("parent_native_thread_id").notNull(),
		agent_native_thread_id: text("agent_native_thread_id").notNull(),
		agent_id: text("agent_id").notNull(),
		assignment_id: text("assignment_id").notNull(),
		run_id: text("run_id").notNull(),
		agent_path: text("agent_path"),
		activity: text("activity"),
		turn_id: text("turn_id"),
		state: text("state").notNull(),
		raw_origin_json: text("raw_origin_json").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("native_subagent_bindings_identity_unique").on(
			table.engine_id,
			table.root_run_id,
			table.agent_native_thread_id,
		),
		index("native_subagent_bindings_group_id_index").on(table.group_id),
		index("native_subagent_bindings_root_run_id_index").on(table.root_run_id),
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
