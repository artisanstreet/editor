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

import {
	workspace_diff_context_lines,
	workspace_diff_format_version,
	workspace_diff_maximum_bytes,
	workspace_diff_maximum_lines_per_side,
	workspace_text_maximum_bytes,
} from "@artisan/protocol";

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

/** Singleton writer lease used only to serialize deterministic projection rebuilds. */
export const ProjectionRebuildLocks = sqliteTable("projection_rebuild_locks", {
	lock_id: integer("lock_id").primaryKey(),
	generation: integer("generation").notNull(),
});

/** Preserves pre-diff migration provenance after the old projection row is rebuilt. */
export const LegacyWorkspaceChangeProjections = sqliteTable("legacy_workspace_change_projections", {
	change_id: text("change_id").primaryKey(),
	source_command_id: text("source_command_id").notNull(),
	thread_id: text("thread_id").notNull(),
});

export const JournalEvents = sqliteTable(
	"journal_events",
	{
		sequence: integer("sequence").primaryKey({ autoIncrement: true }),
		stream_id: text("stream_id").notNull(),
		stream_sequence: integer("stream_sequence").notNull(),
		schema_version: integer("schema_version").notNull(),
		event_id: text("event_id").notNull(),
		idempotency_key: text("idempotency_key"),
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
		uniqueIndex("journal_events_idempotency_key_unique").on(table.idempotency_key),
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

/** Stores the authoritative catalog of projects attached to this Forge instance. */
export const Projects = sqliteTable(
	"projects",
	{
		attached_at: text("attached_at").notNull(),
		display_name: text("display_name").notNull(),
		project_id: text("project_id").primaryKey(),
		root_path: text("root_path").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [uniqueIndex("projects_root_path_unique").on(table.root_path)],
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

/** Canonical Routine records; provider formats remain import or mirror evidence. */
export const MarketplaceRoutines = sqliteTable(
	"marketplace_routines",
	{
		id: text("id").primaryKey(),
		display_name: text("display_name").notNull(),
		description: text("description").notNull(),
		instructions: text("instructions").notNull(),
		source_json: text("source_json").notNull(),
		version: text("version").notNull(),
		author: text("author"),
		scope_json: text("scope_json").notNull(),
		permissions_json: text("permissions_json").notNull(),
		compatibility_json: text("compatibility_json").notNull(),
		commands_json: text("commands_json").notNull(),
		files_json: text("files_json").notNull(),
		trust: text("trust").notNull(),
		enabled: integer("enabled", { mode: "boolean" }).notNull(),
		status: text("status").notNull(),
		artifact_refs_json: text("artifact_refs_json").notNull().default("[]"),
		removed_at: text("removed_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("marketplace_routines_scope_index").on(table.scope_json),
		index("marketplace_routines_enabled_index").on(table.enabled),
	],
);

/** Explicit approval-bound mutations and their canonical lifecycle ledger. */
export const MarketplaceRoutineOperations = sqliteTable(
	"marketplace_routine_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		kind: text("kind").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		approval_id: text("approval_id"),
		approval_fingerprint: text("approval_fingerprint"),
		approval_decision: text("approval_decision"),
		routine_id: text("routine_id"),
		preview_json: text("preview_json"),
		/** Exact reviewed canonical detail for approval recovery; never secret material. */
		detail_json: text("detail_json"),
		rollback_json: text("rollback_json"),
		/** A bounded durable ownership lease prevents concurrent provider effects. */
		dispatch_owner_id: text("dispatch_owner_id"),
		dispatch_lease_expires_at: text("dispatch_lease_expires_at"),
		state: text("state").notNull(),
		failure_code: text("failure_code"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_routine_operations_approval_unique").on(table.approval_id),
		index("marketplace_routine_operations_routine_index").on(table.routine_id),
	],
);

/** Per-engine native mirror state; runtime-only is durable and never a silent sync. */
export const MarketplaceRoutineMirrors = sqliteTable(
	"marketplace_routine_mirrors",
	{
		routine_id: text("routine_id").notNull(),
		engine_id: text("engine_id").notNull(),
		status: text("status").notNull(),
		observed_revision: text("observed_revision"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.routine_id, table.engine_id] })],
);

/** Canonical MCP capability records. Secrets are always opaque references held by the vault. */
export const MarketplaceCapabilities = sqliteTable(
	"marketplace_capabilities",
	{
		id: text("id").primaryKey(),
		display_name: text("display_name").notNull(),
		source_json: text("source_json").notNull(),
		transport_json: text("transport_json").notNull(),
		auth_json: text("auth_json").notNull(),
		scope_json: text("scope_json").notNull(),
		permissions_json: text("permissions_json").notNull(),
		compatibility_json: text("compatibility_json").notNull(),
		tools_json: text("tools_json").notNull(),
		resources_json: text("resources_json").notNull(),
		instructions: text("instructions"),
		policy_json: text("policy_json").notNull(),
		trust: text("trust").notNull(),
		enabled: integer("enabled", { mode: "boolean" }).notNull(),
		status: text("status").notNull(),
		lifecycle: text("lifecycle").notNull(),
		health_json: text("health_json").notNull(),
		raw_provider_metadata_json: text("raw_provider_metadata_json"),
		removed_at: text("removed_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("marketplace_capabilities_scope_index").on(table.scope_json),
		index("marketplace_capabilities_enabled_index").on(table.enabled),
		index("marketplace_capabilities_lifecycle_index").on(table.lifecycle),
	],
);

/** Every connection, approval, OAuth and invocation action is durable and idempotent. */
export const MarketplaceCapabilityOperations = sqliteTable(
	"marketplace_capability_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		capability_id: text("capability_id").notNull(),
		kind: text("kind").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		approval_id: text("approval_id"),
		approval_fingerprint: text("approval_fingerprint"),
		approval_decision: text("approval_decision"),
		preview_json: text("preview_json"),
		/** Exact reviewed canonical detail for connect recovery; never secret material. */
		detail_json: text("detail_json"),
		state: text("state").notNull(),
		failure_code: text("failure_code"),
		artifact_id: text("artifact_id"),
		tool_name: text("tool_name"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_capability_operations_approval_unique").on(table.approval_id),
		index("marketplace_capability_operations_capability_index").on(table.capability_id),
	],
);

/** Bounded MCP tool results referenced by the invocation ledger; never embedded in events. */
export const MarketplaceCapabilityArtifacts = sqliteTable(
	"marketplace_capability_artifacts",
	{
		artifact_id: text("artifact_id").primaryKey(),
		capability_id: text("capability_id").notNull(),
		operation_id: text("operation_id").notNull(),
		tool_name: text("tool_name").notNull(),
		result_json: text("result_json").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_capability_artifacts_operation_unique").on(table.operation_id),
		index("marketplace_capability_artifacts_capability_index").on(table.capability_id),
	],
);

/** Native provider config is a mirror, never an alternate canonical registry. */
export const MarketplaceCapabilityMirrors = sqliteTable(
	"marketplace_capability_mirrors",
	{
		capability_id: text("capability_id").notNull(),
		engine_id: text("engine_id").notNull(),
		status: text("status").notNull(),
		observed_revision: text("observed_revision"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.capability_id, table.engine_id] })],
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
		reviewer_agent_id: text("reviewer_agent_id"),
		reviewer_kind: text("reviewer_kind"),
		reviewer_run_id: text("reviewer_run_id"),
		reviewer_assignment_id: text("reviewer_assignment_id"),
		reviewer_group_id: text("reviewer_group_id"),
		review_outcome: text("review_outcome"),
		review_comment: text("review_comment"),
		workspace_id: text("workspace_id"),
		path: text("path"),
		expected_identity_json: text("expected_identity_json"),
		result_identity_json: text("result_identity_json"),
		diff_format_version: integer("diff_format_version")
			.notNull()
			.default(workspace_diff_format_version),
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
		check(
			"workspace_change_operations_diff_format_version_check",
			sql`${table.diff_format_version} = ${sql.raw(String(workspace_diff_format_version))}`,
		),
		check(
			"workspace_change_operations_reviewer_shape_check",
			sql`(${table.reviewer_kind} IS NULL AND ${table.reviewer_agent_id} IS NULL AND ${table.reviewer_run_id} IS NULL AND ${table.reviewer_assignment_id} IS NULL AND ${table.reviewer_group_id} IS NULL) OR (${table.reviewer_kind} = 'user' AND ${table.reviewer_agent_id} IS NULL AND ${table.reviewer_run_id} IS NULL AND ${table.reviewer_assignment_id} IS NULL AND ${table.reviewer_group_id} IS NULL) OR (${table.reviewer_kind} = 'graph' AND ${table.reviewer_agent_id} IS NOT NULL AND ${table.reviewer_run_id} IS NOT NULL AND ${table.reviewer_assignment_id} IS NOT NULL AND ${table.reviewer_group_id} IS NOT NULL)`,
		),
		check(
			"workspace_change_operations_review_metadata_check",
			sql`(${table.review_outcome} IS NULL OR ${table.review_outcome} IN ('approved', 'changes_requested')) AND (${table.review_comment} IS NULL OR length(${table.review_comment}) <= 4096)`,
		),
		check(
			"workspace_change_operations_raw_origin_check",
			sql`${table.raw_origin_json} IS NULL OR json_valid(${table.raw_origin_json})`,
		),
	],
);

/** Pins the authority snapshot that admitted one controlled replacement claim. */
export const WorkspaceMutationAuthorities = sqliteTable(
	"workspace_mutation_authorities",
	{
		message_id: text("message_id")
			.primaryKey()
			.references(() => WorkspaceChangeOperations.message_id, { onDelete: "cascade" }),
		change_id: text("change_id").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		agent_id: text("agent_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		authority_kind: text("authority_kind").notNull(),
		working_directory: text("working_directory").notNull(),
		group_id: text("group_id"),
		assignment_id: text("assignment_id"),
		scope_kind: text("scope_kind"),
		scope_value: text("scope_value"),
		approval: text("approval"),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_mutation_authorities_change_id_unique").on(table.change_id),
		index("workspace_mutation_authorities_thread_id_index").on(table.thread_id),
		index("workspace_mutation_authorities_run_id_index").on(table.run_id),
		check(
			"workspace_mutation_authorities_shape_check",
			sql`
				(
					${table.authority_kind} = 'base_run'
					AND ${table.group_id} IS NULL
					AND ${table.assignment_id} IS NULL
					AND ${table.scope_kind} IS NULL
					AND ${table.scope_value} IS NULL
					AND ${table.approval} IS NULL
				)
				OR (
					${table.authority_kind} = 'graph_run'
					AND ${table.group_id} IS NOT NULL
					AND ${table.assignment_id} IS NOT NULL
					AND ${table.scope_kind} IN ('repo', 'files')
					AND ${table.scope_value} IS NOT NULL
					AND ${table.approval} IN ('never', 'on_request', 'always')
				)
			`,
		),
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
		review_source_command_id: text("review_source_command_id"),
		reviewer_agent_id: text("reviewer_agent_id"),
		reviewer_kind: text("reviewer_kind"),
		reviewer_run_id: text("reviewer_run_id"),
		reviewer_assignment_id: text("reviewer_assignment_id"),
		reviewer_group_id: text("reviewer_group_id"),
		reviewer_raw_origin_json: text("reviewer_raw_origin_json"),
		review_outcome: text("review_outcome"),
		review_comment: text("review_comment"),
		rolled_back_at: text("rolled_back_at"),
		version: integer("version").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		diff_state: text("diff_state").notNull().default("legacy_unavailable"),
	},
	(table) => [
		uniqueIndex("workspace_changes_source_command_unique").on(table.source_command_id),
		index("workspace_changes_thread_id_index").on(table.thread_id),
		index("workspace_changes_thread_workspace_index").on(table.thread_id, table.workspace_id),
		check(
			"workspace_changes_diff_state_check",
			sql`${table.diff_state} IN ('available', 'legacy_unavailable')`,
		),
		check(
			"workspace_changes_reviewer_shape_check",
			sql`(${table.reviewer_kind} IS NULL AND ${table.reviewer_agent_id} IS NULL AND ${table.reviewer_run_id} IS NULL AND ${table.reviewer_assignment_id} IS NULL AND ${table.reviewer_group_id} IS NULL) OR (${table.reviewer_kind} = 'user' AND ${table.reviewer_agent_id} IS NULL AND ${table.reviewer_run_id} IS NULL AND ${table.reviewer_assignment_id} IS NULL AND ${table.reviewer_group_id} IS NULL) OR (${table.reviewer_kind} = 'graph' AND ${table.reviewer_agent_id} IS NOT NULL AND ${table.reviewer_run_id} IS NOT NULL AND ${table.reviewer_assignment_id} IS NOT NULL AND ${table.reviewer_group_id} IS NOT NULL)`,
		),
		check(
			"workspace_changes_review_metadata_check",
			sql`(${table.review_outcome} IS NULL OR ${table.review_outcome} IN ('approved', 'changes_requested')) AND (${table.review_comment} IS NULL OR length(${table.review_comment}) <= 4096)`,
		),
		check(
			"workspace_changes_reviewer_raw_origin_check",
			sql`${table.reviewer_raw_origin_json} IS NULL OR json_valid(${table.reviewer_raw_origin_json})`,
		),
	],
);

/** Stores source-free, idempotent workspace contention projections. */
export const WorkspaceConflicts = sqliteTable(
	"workspace_conflicts",
	{
		conflict_id: text("conflict_id").primaryKey(),
		source_command_id: text("source_command_id").notNull().unique(),
		change_id: text("change_id").notNull(),
		attempting_thread_id: text("attempting_thread_id").notNull(),
		attempting_run_id: text("attempting_run_id").notNull(),
		attempting_agent_id: text("attempting_agent_id").notNull(),
		assignment_id: text("assignment_id"),
		group_id: text("group_id"),
		workspace_id: text("workspace_id").notNull(),
		path: text("path").notNull(),
		expected_identity_json: text("expected_identity_json").notNull(),
		observed_identity_json: text("observed_identity_json"),
		competing_change_id: text("competing_change_id"),
		raw_origin_json: text("raw_origin_json"),
		resolution: text("resolution").notNull(),
		detected_at: text("detected_at").notNull(),
	},
	(table) => [
		index("workspace_conflicts_thread_index").on(table.attempting_thread_id),
		index("workspace_conflicts_change_index").on(table.change_id),
		check(
			"workspace_conflicts_resolution_check",
			sql`${table.resolution} IN ('rejected', 'reconciled', 'user_action_required')`,
		),
		check(
			"workspace_conflicts_identity_check",
			sql`
				json_valid(${table.expected_identity_json})
				AND json_extract(${table.expected_identity_json}, '$.algorithm') = 'sha256'
				AND json_type(${table.expected_identity_json}, '$.byte_count') = 'integer'
				AND json_extract(${table.expected_identity_json}, '$.byte_count') >= 0
				AND length(json_extract(${table.expected_identity_json}, '$.content_hash')) = 64
				AND json_extract(${table.expected_identity_json}, '$.content_hash') NOT GLOB '*[^0-9a-f]*'
				AND (
					${table.observed_identity_json} IS NULL
					OR (
						json_valid(${table.observed_identity_json})
						AND json_extract(${table.observed_identity_json}, '$.algorithm') = 'sha256'
						AND json_type(${table.observed_identity_json}, '$.byte_count') = 'integer'
						AND json_extract(${table.observed_identity_json}, '$.byte_count') >= 0
						AND length(json_extract(${table.observed_identity_json}, '$.content_hash')) = 64
						AND json_extract(${table.observed_identity_json}, '$.content_hash') NOT GLOB '*[^0-9a-f]*'
					)
				)
			`,
		),
		check(
			"workspace_conflicts_raw_origin_check",
			sql`${table.raw_origin_json} IS NULL OR json_valid(${table.raw_origin_json})`,
		),
	],
);

/** Stores immutable private unified patches independently from projections and journal records. */
export const WorkspaceChangeDiffs = sqliteTable(
	"workspace_change_diffs",
	{
		change_id: text("change_id")
			.primaryKey()
			.references(() => WorkspaceChanges.change_id, { onDelete: "cascade" }),
		source_command_id: text("source_command_id")
			.notNull()
			.references(() => WorkspaceChangeOperations.message_id, { onDelete: "cascade" }),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		path: text("path").notNull(),
		before_identity_json: text("before_identity_json").notNull(),
		after_identity_json: text("after_identity_json").notNull(),
		format: text("format").notNull(),
		format_version: integer("format_version").notNull(),
		context_lines: integer("context_lines").notNull(),
		patch: blob("patch", { mode: "buffer" }).notNull(),
		patch_byte_count: integer("patch_byte_count").notNull(),
		patch_hash: text("patch_hash").notNull(),
		added_line_count: integer("added_line_count").notNull(),
		removed_line_count: integer("removed_line_count").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_change_diffs_source_command_unique").on(table.source_command_id),
		index("workspace_change_diffs_thread_id_index").on(table.thread_id),
		check("workspace_change_diffs_format_check", sql`${table.format} = 'unified'`),
		check(
			"workspace_change_diffs_format_version_check",
			sql`${table.format_version} = ${sql.raw(String(workspace_diff_format_version))}`,
		),
		check(
			"workspace_change_diffs_context_check",
			sql`${table.context_lines} = ${sql.raw(String(workspace_diff_context_lines))}`,
		),
		check(
			"workspace_change_diffs_patch_check",
			sql`
				length(${table.patch}) = ${table.patch_byte_count}
				AND ${table.patch_byte_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_bytes))}
				AND length(${table.patch_hash}) = 64
				AND ${table.patch_hash} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_change_diffs_line_count_check",
			sql`
				${table.added_line_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_lines_per_side))}
				AND ${table.removed_line_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_lines_per_side))}
			`,
		),
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

/** Stores transient exact mutation bytes outside journal and change projections. */
export const WorkspaceMutationPayloads = sqliteTable(
	"workspace_mutation_payloads",
	{
		message_id: text("message_id")
			.primaryKey()
			.references(() => WorkspaceChangeOperations.message_id, { onDelete: "cascade" }),
		thread_id: text("thread_id").notNull(),
		state: text("state").notNull(),
		expected: blob("expected", { mode: "buffer" }),
		expected_byte_count: integer("expected_byte_count"),
		expected_hash: text("expected_hash"),
		replacement: blob("replacement", { mode: "buffer" }),
		replacement_byte_count: integer("replacement_byte_count"),
		replacement_hash: text("replacement_hash"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("workspace_mutation_payloads_thread_id_index").on(table.thread_id),
		check(
			"workspace_mutation_payloads_state_check",
			sql`${table.state} IN ('available', 'consumed')`,
		),
		check(
			"workspace_mutation_payloads_content_check",
			sql`
				(
					${table.state} = 'available'
					AND ${table.expected} IS NOT NULL
					AND ${table.expected_byte_count} IS NOT NULL
					AND ${table.expected_hash} IS NOT NULL
					AND length(${table.expected}) = ${table.expected_byte_count}
					AND ${table.expected_byte_count} BETWEEN 0 AND ${sql.raw(String(workspace_text_maximum_bytes))}
					AND length(${table.expected_hash}) = 64
					AND ${table.expected_hash} NOT GLOB '*[^0-9a-f]*'
					AND ${table.replacement} IS NOT NULL
					AND ${table.replacement_byte_count} IS NOT NULL
					AND ${table.replacement_hash} IS NOT NULL
					AND length(${table.replacement}) = ${table.replacement_byte_count}
					AND ${table.replacement_byte_count} BETWEEN 0 AND ${sql.raw(String(workspace_text_maximum_bytes))}
					AND length(${table.replacement_hash}) = 64
					AND ${table.replacement_hash} NOT GLOB '*[^0-9a-f]*'
				)
				OR (
					${table.state} = 'consumed'
					AND ${table.expected} IS NULL
					AND ${table.expected_byte_count} IS NULL
					AND ${table.expected_hash} IS NULL
					AND ${table.replacement} IS NULL
					AND ${table.replacement_byte_count} IS NULL
					AND ${table.replacement_hash} IS NULL
				)
			`,
		),
	],
);

/** Stores the latest bounded, content-free Git session projection for one workspace. */
export const GitWorkspaceProjections = sqliteTable(
	"git_workspace_projections",
	{
		workspace_id: text("workspace_id").primaryKey(),
		snapshot_id: text("snapshot_id").notNull(),
		version: integer("version").notNull(),
		projection_json: text("projection_json").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		observed_at: text("observed_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		check(
			"git_workspace_projections_snapshot_id_check",
			sql`length(${table.snapshot_id}) = 64 AND ${table.snapshot_id} NOT GLOB '*[^0-9a-f]*'`,
		),
		check("git_workspace_projections_version_check", sql`${table.version} > 0`),
		check(
			"git_workspace_projections_journal_sequence_check",
			sql`${table.journal_sequence} > 0`,
		),
	],
);

/** Stores approval-bound, at-most-once Git index mutation lifecycles. */
export const GitMutationOperations = sqliteTable(
	"git_mutation_operations",
	{
		mutation_id: text("mutation_id").primaryKey(),
		approval_id: text("approval_id").notNull(),
		source_message_id: text("source_message_id").notNull(),
		decision_message_id: text("decision_message_id"),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id"),
		agent_id: text("agent_id"),
		raw_origin_json: text("raw_origin_json"),
		workspace_id: text("workspace_id").notNull(),
		kind: text("kind").notNull(),
		paths_json: text("paths_json").notNull(),
		expected_snapshot_id: text("expected_snapshot_id").notNull(),
		expected_workspace_version: integer("expected_workspace_version").notNull(),
		lifecycle: text("lifecycle").notNull(),
		failure_code: text("failure_code"),
		result_snapshot_id: text("result_snapshot_id"),
		result_workspace_version: integer("result_workspace_version"),
		journal_sequence: integer("journal_sequence"),
		requested_at: text("requested_at").notNull(),
		decision_at: text("decision_at"),
		dispatched_at: text("dispatched_at"),
		dispatch_owner_id: text("dispatch_owner_id"),
		dispatch_lease_expires_at: text("dispatch_lease_expires_at"),
		completed_at: text("completed_at"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("git_mutation_operations_approval_unique").on(table.approval_id),
		uniqueIndex("git_mutation_operations_source_message_unique").on(table.source_message_id),
		uniqueIndex("git_mutation_operations_decision_message_unique").on(
			table.decision_message_id,
		),
		uniqueIndex("git_mutation_operations_workspace_dispatch_unique")
			.on(table.workspace_id)
			.where(sql`${table.lifecycle} = 'dispatching'`),
		index("git_mutation_operations_thread_index").on(table.thread_id),
		index("git_mutation_operations_workspace_index").on(table.workspace_id, table.lifecycle),
		check(
			"git_mutation_operations_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"git_mutation_operations_expected_snapshot_check",
			sql`length(${table.expected_snapshot_id}) = 64 AND ${table.expected_snapshot_id} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"git_mutation_operations_result_snapshot_check",
			sql`${table.result_snapshot_id} IS NULL OR (length(${table.result_snapshot_id}) = 64 AND ${table.result_snapshot_id} NOT GLOB '*[^0-9a-f]*')`,
		),
		check("git_mutation_operations_kind_check", sql`${table.kind} IN ('stage', 'unstage')`),
		check(
			"git_mutation_operations_lifecycle_check",
			sql`${table.lifecycle} IN ('awaiting_approval', 'denied', 'approved', 'dispatching', 'succeeded', 'failed', 'ambiguous')`,
		),
		check(
			"git_mutation_operations_version_check",
			sql`${table.expected_workspace_version} > 0 AND (${table.result_workspace_version} IS NULL OR ${table.result_workspace_version} > 0)`,
		),
	],
);

/** Stores one idempotent, observable built-in or normalized engine action without raw tool input. */
export const ArtisanToolInvocations = sqliteTable(
	"artisan_tool_invocations",
	{
		invocation_id: text("invocation_id").primaryKey(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id"),
		agent_id: text("agent_id"),
		tool_id: text("tool_id").notNull(),
		input_summary: text("input_summary").notNull(),
		execution_input_json: text("execution_input_json").notNull(),
		permission_json: text("permission_json").notNull(),
		raw_origin_json: text("raw_origin_json"),
		approval_id: text("approval_id"),
		workspace_evidence_json: text("workspace_evidence_json"),
		lifecycle: text("lifecycle").notNull(),
		outcome_json: text("outcome_json"),
		claim_owner_id: text("claim_owner_id"),
		claim_lease_expires_at: text("claim_lease_expires_at"),
		requested_at: text("requested_at").notNull(),
		completed_at: text("completed_at"),
		updated_at: text("updated_at").notNull(),
		journal_sequence: integer("journal_sequence"),
	},
	(table) => [
		uniqueIndex("artisan_tool_invocations_approval_unique").on(table.approval_id),
		index("artisan_tool_invocations_thread_index").on(table.thread_id, table.requested_at),
		index("artisan_tool_invocations_thread_lifecycle_index").on(
			table.thread_id,
			table.lifecycle,
		),
		check(
			"artisan_tool_invocations_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"artisan_tool_invocations_lifecycle_check",
			sql`${table.lifecycle} IN ('requested', 'awaiting_approval', 'running', 'succeeded', 'denied', 'failed', 'cancelled', 'unsupported')`,
		),
		check(
			"artisan_tool_invocations_terminal_check",
			sql`(${table.lifecycle} IN ('succeeded', 'denied', 'failed', 'cancelled', 'unsupported')) = (${table.outcome_json} IS NOT NULL)`,
		),
	],
);

/** Stores reload-safe approval interactions exactly bound to one tool invocation. */
export const ArtisanToolApprovals = sqliteTable(
	"artisan_tool_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		invocation_id: text("invocation_id").notNull(),
		request_json: text("request_json").notNull(),
		state: text("state").notNull(),
		resolution_json: text("resolution_json"),
		resolution_id: text("resolution_id"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("artisan_tool_approvals_invocation_unique").on(table.invocation_id),
		uniqueIndex("artisan_tool_approvals_resolution_unique").on(table.resolution_id),
		index("artisan_tool_approvals_state_index").on(table.state),
		check("artisan_tool_approvals_state_check", sql`${table.state} IN ('pending', 'resolved')`),
		check(
			"artisan_tool_approvals_resolution_check",
			sql`(${table.state} = 'resolved') = (${table.resolution_json} IS NOT NULL AND ${table.resolution_id} IS NOT NULL)`,
		),
	],
);

/** Durable local-only preview targets, retained independently of process ownership. */
export const PreviewTargets = sqliteTable(
	"preview_targets",
	{
		target_id: text("target_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		project_id: text("project_id").notNull(),
		url: text("url").notNull(),
		port: integer("port").notNull(),
		routes_json: text("routes_json").notNull(),
		source_kind: text("source_kind"),
		source_id: text("source_id"),
		state: text("state").notNull(),
		launch_state: text("launch_state").notNull(),
		last_error: text("last_error"),
		health_json: text("health_json"),
		journal_sequence: integer("journal_sequence").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		removed_at: text("removed_at"),
	},
	(table) => [
		index("preview_targets_thread_id_index").on(table.thread_id),
		index("preview_targets_workspace_id_index").on(table.workspace_id),
		check(
			"preview_targets_state_check",
			sql`${table.state} IN ('registered', 'healthy', 'unhealthy', 'stopped', 'removed')`,
		),
		check("preview_targets_port_check", sql`${table.port} BETWEEN 1 AND 65535`),
		check(
			"preview_targets_launch_state_check",
			sql`${table.launch_state} IN ('idle', 'launching', 'launched', 'unavailable', 'error')`,
		),
		check(
			"preview_targets_source_check",
			sql`(${table.source_kind} IS NULL AND ${table.source_id} IS NULL) OR (${table.source_kind} IN ('process', 'terminal') AND ${table.source_id} IS NOT NULL)`,
		),
		check("preview_targets_journal_sequence_check", sql`${table.journal_sequence} > 0`),
	],
);

/** Explicit, attributable external-browser inspection sessions. */
export const PreviewInspectionSessions = sqliteTable(
	"preview_inspection_sessions",
	{
		session_id: text("session_id").primaryKey(),
		target_id: text("target_id").notNull(),
		thread_id: text("thread_id").notNull(),
		connector_id: text("connector_id").notNull(),
		state: text("state").notNull(),
		reconnect_state: text("reconnect_state").notNull(),
		last_error: text("last_error"),
		journal_sequence: integer("journal_sequence").notNull(),
		opened_at: text("opened_at").notNull(),
		closed_at: text("closed_at"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("preview_inspection_sessions_thread_id_index").on(table.thread_id),
		index("preview_inspection_sessions_target_id_index").on(table.target_id),
		check(
			"preview_inspection_sessions_state_check",
			sql`${table.state} IN ('open', 'closed', 'abandoned')`,
		),
		check(
			"preview_inspection_sessions_reconnect_state_check",
			sql`${table.reconnect_state} IN ('connected', 'reconnecting', 'unavailable', 'error')`,
		),
		check(
			"preview_inspection_sessions_closed_check",
			sql`(${table.state} = 'open' AND ${table.closed_at} IS NULL) OR (${table.state} IN ('closed', 'abandoned') AND ${table.closed_at} IS NOT NULL)`,
		),
		check(
			"preview_inspection_sessions_journal_sequence_check",
			sql`${table.journal_sequence} > 0`,
		),
	],
);

/** Exact command identities for preview projection changes. */
export const PreviewCommands = sqliteTable(
	"preview_commands",
	{
		message_id: text("message_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		action: text("action").notNull(),
		payload_json: text("payload_json").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		index("preview_commands_thread_id_index").on(table.thread_id),
		check(
			"preview_commands_action_check",
			sql`${table.action} IN ('register', 'probe', 'state', 'remove', 'launch', 'inspection_open', 'inspection_reconnect', 'inspection_close', 'recovery')`,
		),
		check("preview_commands_journal_sequence_check", sql`${table.journal_sequence} > 0`),
	],
);

/** A bounded cross-runtime ownership lease around a preview side effect. */
export const PreviewDispatchLeases = sqliteTable(
	"preview_dispatch_leases",
	{
		lease_id: text("lease_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		kind: text("kind").notNull(),
		target_id: text("target_id"),
		session_id: text("session_id"),
		acquired_at: text("acquired_at").notNull(),
		expires_at: text("expires_at").notNull(),
	},
	(table) => [
		uniqueIndex("preview_dispatch_leases_thread_id_unique").on(table.thread_id),
		index("preview_dispatch_leases_expires_at_index").on(table.expires_at),
		check(
			"preview_dispatch_leases_kind_check",
			sql`${table.kind} IN ('launch', 'probe', 'inspection_open', 'inspection_health')`,
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
	auto_steer_follow_ups: integer("auto_steer_follow_ups", { mode: "boolean" })
		.notNull()
		.default(true),
	policy_model: text("policy_model"),
	policy_reasoning_effort: text("policy_reasoning_effort").notNull().default("medium"),
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

/** Safe, ordered projection of one raw engine observation. */
export const SurfaceItems = sqliteTable(
	"surface_items",
	{
		projection_order: integer("projection_order").primaryKey({ autoIncrement: true }),
		surface_id: text("surface_id").notNull().unique(),
		observation_id: text("observation_id").notNull().unique(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		group_id: text("group_id"),
		assignment_id: text("assignment_id"),
		sequence: integer("sequence").notNull(),
		category: text("category").notNull(),
		kind: text("kind").notNull(),
		summary_json: text("summary_json").notNull(),
		raw_origin_json: text("raw_origin_json"),
		occurred_at: text("occurred_at").notNull(),
	},
	(table) => [
		index("surface_items_thread_projection_order_index").on(
			table.thread_id,
			table.projection_order,
		),
	],
);

/** Optional provider-neutral token totals; null means unavailable, never zero-by-invention. */
export const SurfaceUsageTotals = sqliteTable("surface_usage_totals", {
	run_id: text("run_id").primaryKey(),
	group_id: text("group_id"),
	assignment_id: text("assignment_id"),
	input_tokens: integer("input_tokens"),
	output_tokens: integer("output_tokens"),
	updated_at: text("updated_at").notNull(),
});

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
		owner_kind: text("owner_kind").notNull().default("user"),
		owner_agent_id: text("owner_agent_id"),
		owner_run_id: text("owner_run_id"),
		pinned: integer("pinned", { mode: "boolean" }).notNull().default(false),
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
		check("terminal_sessions_owner_kind_check", sql`${table.owner_kind} IN ('user', 'agent')`),
		check(
			"terminal_sessions_owner_identity_check",
			sql`(${table.owner_kind} = 'user' AND ${table.owner_agent_id} IS NULL AND ${table.owner_run_id} IS NULL) OR (${table.owner_kind} = 'agent' AND ${table.owner_agent_id} IS NOT NULL AND ${table.owner_run_id} IS NOT NULL)`,
		),
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

/** Renderer-ready canonical conversation state. Source identities make projection replay idempotent. */
export const ConversationThreads = sqliteTable("conversation_threads", {
	thread_id: text("thread_id").primaryKey(),
	next_ordinal: integer("next_ordinal").notNull().default(0),
	last_patch_sequence: integer("last_patch_sequence").notNull().default(0),
	journal_sequence: integer("journal_sequence").notNull().default(0),
	updated_at: text("updated_at").notNull(),
});

export const ConversationTurns = sqliteTable(
	"conversation_turns",
	{
		turn_id: text("turn_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		ordinal: integer("ordinal").notNull(),
		entity_json: text("entity_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_turns_thread_ordinal_unique").on(table.thread_id, table.ordinal),
		index("conversation_turns_thread_index").on(table.thread_id),
	],
);

export const ConversationItems = sqliteTable(
	"conversation_items",
	{
		item_id: text("item_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		turn_id: text("turn_id").notNull(),
		ordinal: integer("ordinal").notNull(),
		entity_json: text("entity_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_items_thread_ordinal_unique").on(table.thread_id, table.ordinal),
		index("conversation_items_thread_turn_index").on(table.thread_id, table.turn_id),
	],
);

export const ConversationPatches = sqliteTable(
	"conversation_patches",
	{
		patch_id: text("patch_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		sequence: integer("sequence").notNull(),
		patch_json: text("patch_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_patches_thread_sequence_unique").on(
			table.thread_id,
			table.sequence,
		),
		index("conversation_patches_thread_index").on(table.thread_id, table.sequence),
	],
);

/** One successful source admission means an exact replay cannot allocate another ordinal or patch. */
export const ConversationSources = sqliteTable("conversation_sources", {
	source_id: text("source_id").primaryKey(),
	thread_id: text("thread_id").notNull(),
	journal_sequence: integer("journal_sequence"),
	observed_at: text("observed_at").notNull(),
});
