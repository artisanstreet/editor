import { sql } from "drizzle-orm";
import {
	blob,
	check,
	foreignKey,
	index,
	integer,
	primaryKey,
	sqliteTable,
	text,
	uniqueIndex,
} from "drizzle-orm/sqlite-core";

import {
	tool_json_maximum_bytes,
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

/** Stores durable approval decisions and private pending diffs for controlled replacements. */
export const WorkspaceReplaceApprovals = sqliteTable(
	"workspace_replace_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		message_id: text("message_id")
			.notNull()
			.references(() => WorkspaceChangeOperations.message_id, { onDelete: "cascade" }),
		request_fingerprint: text("request_fingerprint").notNull(),
		operation_sent_at: text("operation_sent_at").notNull(),
		change_id: text("change_id").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		agent_id: text("agent_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		path: text("path").notNull(),
		before_identity_json: text("before_identity_json").notNull(),
		after_identity_json: text("after_identity_json").notNull(),
		policy: text("policy").notNull(),
		reason: text("reason").notNull(),
		state: text("state").notNull(),
		decision_message_id: text("decision_message_id"),
		approved: integer("approved", { mode: "boolean" }),
		decided_at: text("decided_at"),
		raw_origin_json: text("raw_origin_json"),
		format: text("format").notNull(),
		format_version: integer("format_version").notNull(),
		context_lines: integer("context_lines").notNull(),
		patch: blob("patch", { mode: "buffer" }).notNull(),
		patch_byte_count: integer("patch_byte_count").notNull(),
		patch_hash: text("patch_hash").notNull(),
		added_line_count: integer("added_line_count").notNull(),
		removed_line_count: integer("removed_line_count").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_replace_approvals_message_id_unique").on(table.message_id),
		uniqueIndex("workspace_replace_approvals_change_id_unique").on(table.change_id),
		uniqueIndex("workspace_replace_approvals_decision_message_unique").on(
			table.decision_message_id,
		),
		index("workspace_replace_approvals_thread_id_index").on(table.thread_id),
		index("workspace_replace_approvals_state_index").on(table.state),
		check(
			"workspace_replace_approvals_policy_check",
			sql`${table.policy} IN ('on_request', 'always')`,
		),
		check(
			"workspace_replace_approvals_request_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_replace_approvals_state_check",
			sql`${table.state} IN ('requested', 'approved', 'executing', 'denied', 'applied', 'rejected')`,
		),
		check(
			"workspace_replace_approvals_decision_check",
			sql`
				(
					${table.state} = 'requested'
					AND ${table.decision_message_id} IS NULL
					AND ${table.approved} IS NULL
					AND ${table.decided_at} IS NULL
				)
				OR (
					${table.state} = 'denied'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 0
					AND ${table.decided_at} IS NOT NULL
				)
				OR (
					${table.state} IN ('approved', 'executing', 'applied', 'rejected')
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
				)
			`,
		),
		check("workspace_replace_approvals_format_check", sql`${table.format} = 'unified'`),
		check(
			"workspace_replace_approvals_format_version_check",
			sql`${table.format_version} = ${sql.raw(String(workspace_diff_format_version))}`,
		),
		check(
			"workspace_replace_approvals_context_check",
			sql`${table.context_lines} = ${sql.raw(String(workspace_diff_context_lines))}`,
		),
		check(
			"workspace_replace_approvals_patch_check",
			sql`
				length(${table.patch}) = ${table.patch_byte_count}
				AND ${table.patch_byte_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_bytes))}
				AND length(${table.patch_hash}) = 64
				AND ${table.patch_hash} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_replace_approvals_line_count_check",
			sql`
				${table.added_line_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_lines_per_side))}
				AND ${table.removed_line_count} BETWEEN 0 AND ${sql.raw(String(workspace_diff_maximum_lines_per_side))}
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

/** Stores the latest source-free Git projection for one registered workspace. */
export const WorkspaceGitSessions = sqliteTable(
	"workspace_git_sessions",
	{
		workspace_id: text("workspace_id").primaryKey(),
		repository_root: text("repository_root"),
		selected_worktree_path: text("selected_worktree_path"),
		state: text("state").notNull(),
		blockers_json: text("blockers_json").notNull(),
		branch: text("branch"),
		head: text("head"),
		additions: integer("additions").notNull(),
		deletions: integer("deletions").notNull(),
		files: integer("files").notNull(),
		has_diff: integer("has_diff", { mode: "boolean" }).notNull(),
		version: integer("version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		observed_at: text("observed_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		check(
			"workspace_git_sessions_state_check",
			sql`${table.state} IN ('ready', 'blocked', 'unavailable')`,
		),
		check(
			"workspace_git_sessions_counts_check",
			sql`
				${table.additions} >= 0
				AND ${table.deletions} >= 0
				AND ${table.files} >= 0
				AND ${table.version} >= 1
				AND ${table.journal_sequence} >= 1
			`,
		),
		check(
			"workspace_git_sessions_repository_shape_check",
			sql`
				(
					${table.state} = 'unavailable'
					AND ${table.repository_root} IS NULL
					AND ${table.selected_worktree_path} IS NULL
				)
				OR (
					${table.state} IN ('ready', 'blocked')
					AND ${table.repository_root} IS NOT NULL
					AND ${table.selected_worktree_path} IS NOT NULL
				)
			`,
		),
	],
);

/** Stores the private worktree inventory behind one public Git-session projection. */
export const WorkspaceGitWorktrees = sqliteTable(
	"workspace_git_worktrees",
	{
		workspace_id: text("workspace_id")
			.notNull()
			.references(() => WorkspaceGitSessions.workspace_id, { onDelete: "cascade" }),
		ordinal: integer("ordinal").notNull(),
		adapter_path: text("adapter_path").notNull(),
		location: text("location").notNull(),
		branch: text("branch"),
		head: text("head"),
		bare: integer("bare", { mode: "boolean" }).notNull(),
		detached: integer("detached", { mode: "boolean" }).notNull(),
		locked: integer("locked", { mode: "boolean" }).notNull(),
		prunable: integer("prunable", { mode: "boolean" }).notNull(),
		version: integer("version").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.workspace_id, table.ordinal] }),
		index("workspace_git_worktrees_workspace_index").on(table.workspace_id),
		check(
			"workspace_git_worktrees_location_check",
			sql`${table.location} IN ('selected', 'external')`,
		),
		check(
			"workspace_git_worktrees_ordinal_version_check",
			sql`${table.ordinal} >= 0 AND ${table.version} >= 1`,
		),
	],
);

/** Stores changed-file facts for the latest Git-session version without patch bytes. */
export const WorkspaceGitChangedFiles = sqliteTable(
	"workspace_git_changed_files",
	{
		workspace_id: text("workspace_id")
			.notNull()
			.references(() => WorkspaceGitSessions.workspace_id, { onDelete: "cascade" }),
		path: text("path").notNull(),
		original_path: text("original_path"),
		status: text("status").notNull(),
		staged: integer("staged", { mode: "boolean" }).notNull(),
		unstaged: integer("unstaged", { mode: "boolean" }).notNull(),
		untracked: integer("untracked", { mode: "boolean" }).notNull(),
		conflicted: integer("conflicted", { mode: "boolean" }).notNull(),
		version: integer("version").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.workspace_id, table.path] }),
		index("workspace_git_changed_files_workspace_index").on(table.workspace_id),
		check("workspace_git_changed_files_version_check", sql`${table.version} >= 1`),
	],
);

/** Stores idempotent Git observations and exact evidence needed after a restart. */
export const WorkspaceGitOperations = sqliteTable(
	"workspace_git_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		source_command_id: text("source_command_id"),
		request_fingerprint: text("request_fingerprint").notNull(),
		kind: text("kind").notNull(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		session_version: integer("session_version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		evidence_recorded: integer("evidence_recorded", { mode: "boolean" })
			.notNull()
			.default(false),
		evidence_root_path: text("evidence_root_path"),
		evidence_worktree_path: text("evidence_worktree_path"),
		evidence_branch: text("evidence_branch"),
		evidence_changed_file_count: integer("evidence_changed_file_count"),
		evidence_has_diff: integer("evidence_has_diff", { mode: "boolean" }),
		sent_at: text("sent_at").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_git_operations_source_command_unique").on(table.source_command_id),
		index("workspace_git_operations_workspace_index").on(table.workspace_id),
		index("workspace_git_operations_pending_evidence_index").on(table.evidence_recorded),
		check(
			"workspace_git_operations_kind_check",
			sql`${table.kind} IN ('refresh', 'checkout', 'recovery', 'mutation')`,
		),
		check(
			"workspace_git_operations_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_git_operations_version_sequence_check",
			sql`${table.session_version} >= 1 AND ${table.journal_sequence} >= 1`,
		),
		check(
			"workspace_git_operations_evidence_check",
			sql`
				(
					${table.evidence_recorded} = 1
				)
				OR (
					${table.evidence_root_path} IS NOT NULL
					AND ${table.evidence_worktree_path} IS NOT NULL
					AND ${table.evidence_changed_file_count} IS NOT NULL
					AND ${table.evidence_changed_file_count} >= 0
					AND ${table.evidence_has_diff} IS NOT NULL
				)
			`,
		),
	],
);

/** Stores source-free approval state for one explicit local-branch checkout request. */
export const WorkspaceGitCheckoutApprovals = sqliteTable(
	"workspace_git_checkout_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		expected_session_version: integer("expected_session_version").notNull(),
		source_branch: text("source_branch").notNull(),
		source_head: text("source_head").notNull(),
		target_branch: text("target_branch").notNull(),
		target_head: text("target_head").notNull(),
		state: text("state").notNull(),
		decision_message_id: text("decision_message_id"),
		approved: integer("approved", { mode: "boolean" }),
		decided_at: text("decided_at"),
		execution_started_at: text("execution_started_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_git_checkout_approvals_source_command_unique").on(
			table.source_command_id,
		),
		uniqueIndex("workspace_git_checkout_approvals_decision_message_unique").on(
			table.decision_message_id,
		),
		index("workspace_git_checkout_approvals_thread_index").on(table.thread_id),
		index("workspace_git_checkout_approvals_state_index").on(table.state),
		check(
			"workspace_git_checkout_approvals_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_git_checkout_approvals_state_check",
			sql`${table.state} IN ('requested', 'approved', 'executing', 'applied', 'denied', 'rejected', 'unknown')`,
		),
		check(
			"workspace_git_checkout_approvals_decision_check",
			sql`
				(
					${table.state} = 'requested'
					AND ${table.decision_message_id} IS NULL
					AND ${table.approved} IS NULL
					AND ${table.decided_at} IS NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'denied'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 0
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'approved'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} IN ('executing', 'applied', 'rejected', 'unknown')
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NOT NULL
				)
			`,
		),
		check(
			"workspace_git_checkout_approvals_version_check",
			sql`${table.expected_session_version} >= 1`,
		),
	],
);

/** Serializes branch checkout with every controlled writer in the visible workspace. */
export const WorkspaceGitCheckoutClaims = sqliteTable(
	"workspace_git_checkout_claims",
	{
		workspace_id: text("workspace_id").primaryKey(),
		approval_id: text("approval_id")
			.notNull()
			.references(() => WorkspaceGitCheckoutApprovals.approval_id, {
				onDelete: "cascade",
			}),
		thread_id: text("thread_id").notNull(),
		claimed_at: text("claimed_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_git_checkout_claims_approval_unique").on(table.approval_id),
		index("workspace_git_checkout_claims_thread_index").on(table.thread_id),
	],
);

/** Stores public approval state with session branch/head but no private plan or proof. */
export const WorkspaceGitMutationApprovals = sqliteTable(
	"workspace_git_mutation_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		expected_session_version: integer("expected_session_version").notNull(),
		action_approval_id: text("action_approval_id"),
		operation_summary_json: text("operation_summary_json").notNull(),
		source_branch: text("source_branch"),
		source_head: text("source_head").notNull(),
		state: text("state").notNull(),
		decision_message_id: text("decision_message_id"),
		approved: integer("approved", { mode: "boolean" }),
		decided_at: text("decided_at"),
		execution_started_at: text("execution_started_at"),
		resulting_branch: text("resulting_branch"),
		resulting_head: text("resulting_head"),
		remote_head: text("remote_head"),
		required_action: text("required_action"),
		rejection_reason: text("rejection_reason"),
		unknown_reason: text("unknown_reason"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("workspace_git_mutation_approvals_source_command_unique").on(
			table.source_command_id,
		),
		uniqueIndex("workspace_git_mutation_approvals_decision_message_unique").on(
			table.decision_message_id,
		),
		index("workspace_git_mutation_approvals_thread_index").on(table.thread_id),
		index("workspace_git_mutation_approvals_state_index").on(table.state),
		index("workspace_git_mutation_approvals_action_parent_index").on(table.action_approval_id),
		check(
			"workspace_git_mutation_approvals_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_git_mutation_approvals_version_check",
			sql`${table.expected_session_version} >= 1`,
		),
		check(
			"workspace_git_mutation_approvals_state_check",
			sql`
				${table.state} IN (
					'requested', 'approved', 'executing', 'applied',
					'action_required', 'rejected', 'outcome_unknown', 'denied'
				)
			`,
		),
		check(
			"workspace_git_mutation_approvals_decision_check",
			sql`
				(
					${table.state} = 'requested'
					AND ${table.decision_message_id} IS NULL
					AND ${table.approved} IS NULL
					AND ${table.decided_at} IS NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'denied'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 0
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'approved'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} IN (
						'executing', 'applied', 'action_required', 'rejected', 'outcome_unknown'
					)
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NOT NULL
				)
			`,
		),
		check(
			"workspace_git_mutation_approvals_outcome_check",
			sql`
				(
					${table.state} IN ('requested', 'approved', 'executing', 'denied')
					AND ${table.resulting_branch} IS NULL
					AND ${table.resulting_head} IS NULL
					AND ${table.remote_head} IS NULL
					AND ${table.required_action} IS NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'applied'
					AND ${table.resulting_head} IS NOT NULL
					AND ${table.required_action} IS NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'action_required'
					AND ${table.required_action} IS NOT NULL
					AND ${table.resulting_branch} IS NULL
					AND ${table.resulting_head} IS NULL
					AND ${table.remote_head} IS NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'rejected'
					AND ${table.rejection_reason} IS NOT NULL
					AND ${table.resulting_branch} IS NULL
					AND ${table.resulting_head} IS NULL
					AND ${table.remote_head} IS NULL
					AND ${table.required_action} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'outcome_unknown'
					AND ${table.unknown_reason} IS NOT NULL
					AND ${table.resulting_branch} IS NULL
					AND ${table.resulting_head} IS NULL
					AND ${table.remote_head} IS NULL
					AND ${table.required_action} IS NULL
					AND ${table.rejection_reason} IS NULL
				)
			`,
		),
	],
);

/** Stores private plan, attempt, and reconciliation evidence for one approval. */
export const WorkspaceGitMutationArtifacts = sqliteTable(
	"workspace_git_mutation_artifacts",
	{
		approval_id: text("approval_id")
			.primaryKey()
			.references(() => WorkspaceGitMutationApprovals.approval_id, {
				onDelete: "cascade",
			}),
		operation_json: text("operation_json").notNull(),
		plan_json: text("plan_json").notNull(),
		plan_binding: text("plan_binding").notNull(),
		attempt_json: text("attempt_json"),
		attempt_binding: text("attempt_binding"),
		reconciliation_json: text("reconciliation_json"),
		reconciled_at: text("reconciled_at"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		check(
			"workspace_git_mutation_artifacts_plan_binding_check",
			sql`
				length(${table.plan_binding}) = 64
				AND ${table.plan_binding} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_git_mutation_artifacts_attempt_binding_check",
			sql`
				(
					${table.attempt_json} IS NULL
					AND ${table.attempt_binding} IS NULL
				)
				OR (
					${table.attempt_json} IS NOT NULL
					AND length(${table.attempt_binding}) = 64
					AND ${table.attempt_binding} NOT GLOB '*[^0-9a-f]*'
				)
			`,
		),
		check(
			"workspace_git_mutation_artifacts_reconciliation_pair_check",
			sql`
				(${table.reconciliation_json} IS NULL) = (${table.reconciled_at} IS NULL)
			`,
		),
	],
);

/** Serializes generic Git mutations with every controlled writer in the visible workspace. */
export const WorkspaceGitMutationClaims = sqliteTable(
	"workspace_git_mutation_claims",
	{
		workspace_id: text("workspace_id").primaryKey(),
		approval_id: text("approval_id")
			.notNull()
			.references(() => WorkspaceGitMutationApprovals.approval_id, {
				onDelete: "cascade",
			}),
		thread_id: text("thread_id").notNull(),
		claim_token: text("claim_token").notNull(),
		claimed_at: text("claimed_at").notNull(),
		owner_instance_id: text("owner_instance_id").notNull().default("legacy_expired"),
		lease_expires_at: text("lease_expires_at").notNull().default("1970-01-01T00:00:00.000Z"),
		execution_started_at: text("execution_started_at"),
		execution_completed_at: text("execution_completed_at"),
	},
	(table) => [
		uniqueIndex("workspace_git_mutation_claims_approval_unique").on(table.approval_id),
		uniqueIndex("workspace_git_mutation_claims_claim_token_unique").on(table.claim_token),
		index("workspace_git_mutation_claims_thread_index").on(table.thread_id),
		index("workspace_git_mutation_claims_lease_index").on(table.lease_expires_at),
	],
);

/** Stores the one user-controlled policy for automatic local Git fetch. */
export const WorkspaceGitFetchPolicies = sqliteTable(
	"workspace_git_fetch_policies",
	{
		policy_id: integer("policy_id").primaryKey(),
		enabled: integer("enabled", { mode: "boolean" }).notNull().default(false),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [check("workspace_git_fetch_policies_singleton_check", sql`${table.policy_id} = 1`)],
);

/** Stores the latest terminal fetch result and, at most, one leased active attempt. */
export const WorkspaceGitFetchStates = sqliteTable(
	"workspace_git_fetch_states",
	{
		workspace_id: text("workspace_id").primaryKey(),
		last_attempted_at: text("last_attempted_at"),
		last_result: text("last_result"),
		version: integer("version").notNull().default(0),
		active_attempt_id: text("active_attempt_id"),
		active_kind: text("active_kind"),
		active_message_id: text("active_message_id"),
		started_at: text("started_at"),
		lease_owner: text("lease_owner"),
		lease_expires_at: text("lease_expires_at"),
	},
	(table) => [
		check(
			"workspace_git_fetch_states_result_check",
			sql`
				(${table.last_attempted_at} IS NULL) = (${table.last_result} IS NULL)
				AND (
					${table.last_result} IS NULL
					OR ${table.last_result} IN ('succeeded', 'failed', 'unavailable')
				)
			`,
		),
		check("workspace_git_fetch_states_version_check", sql`${table.version} >= 0`),
		check(
			"workspace_git_fetch_states_active_check",
			sql`
				(
					${table.active_attempt_id} IS NULL
					AND ${table.active_kind} IS NULL
					AND ${table.active_message_id} IS NULL
					AND ${table.started_at} IS NULL
					AND ${table.lease_owner} IS NULL
					AND ${table.lease_expires_at} IS NULL
				)
				OR (
					${table.active_attempt_id} IS NOT NULL
					AND ${table.active_kind} = 'automatic'
					AND ${table.active_message_id} IS NULL
					AND ${table.started_at} IS NOT NULL
					AND ${table.lease_owner} IS NOT NULL
					AND ${table.lease_expires_at} IS NOT NULL
				)
				OR (
					${table.active_attempt_id} IS NOT NULL
					AND ${table.active_kind} = 'manual'
					AND ${table.active_message_id} IS NOT NULL
					AND ${table.started_at} IS NOT NULL
					AND ${table.lease_owner} IS NOT NULL
					AND ${table.lease_expires_at} IS NOT NULL
				)
			`,
		),
	],
);

/** Stores exact replay state for sparse policy updates and manual fetch requests. */
export const WorkspaceGitFetchOperations = sqliteTable(
	"workspace_git_fetch_operations",
	{
		message_id: text("message_id").primaryKey(),
		kind: text("kind").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		sent_at: text("sent_at").notNull(),
		enabled: integer("enabled", { mode: "boolean" }),
		thread_id: text("thread_id"),
		workspace_id: text("workspace_id"),
		attempt_id: text("attempt_id"),
		status: text("status").notNull(),
		result: text("result"),
		attempted_at: text("attempted_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("workspace_git_fetch_operations_pending_index").on(
			table.workspace_id,
			table.status,
			table.created_at,
		),
		check(
			"workspace_git_fetch_operations_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"workspace_git_fetch_operations_shape_check",
			sql`
				(
					${table.kind} = 'policy'
					AND ${table.enabled} IS NOT NULL
					AND ${table.thread_id} IS NULL
					AND ${table.workspace_id} IS NULL
					AND ${table.attempt_id} IS NULL
					AND ${table.status} = 'terminal'
					AND ${table.result} IS NULL
					AND ${table.attempted_at} IS NULL
				)
				OR (
					${table.kind} = 'manual'
					AND ${table.enabled} IS NULL
					AND ${table.thread_id} IS NOT NULL
					AND ${table.workspace_id} IS NOT NULL
					AND ${table.attempt_id} IS NOT NULL
					AND (
						(${table.status} = 'pending' AND ${table.result} IS NULL AND ${table.attempted_at} IS NULL)
						OR (
							${table.status} = 'terminal'
							AND ${table.result} IN ('succeeded', 'failed', 'unavailable')
							AND ${table.attempted_at} IS NOT NULL
						)
					)
				)
			`,
		),
	],
);

/** Stores public, source-safe hosted clone approval lifecycles. */
export const HostedProjectCloneApprovals = sqliteTable(
	"hosted_project_clone_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		destination_path: text("destination_path").notNull(),
		repository_json: text("repository_json").notNull(),
		state: text("state").notNull(),
		decision_message_id: text("decision_message_id"),
		approved: integer("approved", { mode: "boolean" }),
		decided_at: text("decided_at"),
		execution_started_at: text("execution_started_at"),
		project_json: text("project_json"),
		attachment: text("attachment"),
		rejection_reason: text("rejection_reason"),
		unknown_reason: text("unknown_reason"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("hosted_project_clone_approvals_source_command_unique").on(
			table.source_command_id,
		),
		uniqueIndex("hosted_project_clone_approvals_decision_message_unique").on(
			table.decision_message_id,
		),
		index("hosted_project_clone_approvals_thread_index").on(table.thread_id),
		index("hosted_project_clone_approvals_state_index").on(table.state),
		check(
			"hosted_project_clone_approvals_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"hosted_project_clone_approvals_state_check",
			sql`${table.state} IN ('requested', 'reused', 'approved', 'executing', 'applied', 'attachment_conflict', 'rejected', 'outcome_unknown', 'denied')`,
		),
		check(
			"hosted_project_clone_approvals_decision_check",
			sql`
			(${table.state} IN ('requested', 'reused') AND ${table.decision_message_id} IS NULL AND ${table.approved} IS NULL AND ${table.decided_at} IS NULL AND ${table.execution_started_at} IS NULL)
			OR (${table.state} = 'denied' AND ${table.decision_message_id} IS NOT NULL AND ${table.approved} = 0 AND ${table.decided_at} IS NOT NULL AND ${table.execution_started_at} IS NULL)
			OR (${table.state} = 'approved' AND ${table.decision_message_id} IS NOT NULL AND ${table.approved} = 1 AND ${table.decided_at} IS NOT NULL AND ${table.execution_started_at} IS NULL)
			OR (${table.state} IN ('executing', 'applied', 'attachment_conflict', 'rejected', 'outcome_unknown') AND ${table.decision_message_id} IS NOT NULL AND ${table.approved} = 1 AND ${table.decided_at} IS NOT NULL AND ${table.execution_started_at} IS NOT NULL)
		`,
		),
		check(
			"hosted_project_clone_approvals_outcome_check",
			sql`
			(${table.state} IN ('requested', 'approved', 'executing', 'denied') AND ${table.project_json} IS NULL AND ${table.attachment} IS NULL AND ${table.rejection_reason} IS NULL AND ${table.unknown_reason} IS NULL)
			OR (${table.state} = 'reused' AND ${table.project_json} IS NOT NULL AND ${table.attachment} IN ('attached', 'already_attached') AND ${table.rejection_reason} IS NULL AND ${table.unknown_reason} IS NULL)
			OR (${table.state} = 'applied' AND ${table.project_json} IS NOT NULL AND ${table.attachment} IN ('attached', 'already_attached') AND ${table.rejection_reason} IS NULL AND ${table.unknown_reason} IS NULL)
			OR (${table.state} = 'attachment_conflict' AND ${table.project_json} IS NOT NULL AND ${table.attachment} IS NULL AND ${table.rejection_reason} IS NULL AND ${table.unknown_reason} IS NULL)
			OR (${table.state} = 'rejected' AND ${table.project_json} IS NULL AND ${table.attachment} IS NULL AND ${table.rejection_reason} IN ('destination_unavailable', 'provider_unavailable', 'repository_unavailable', 'thread_unavailable') AND ${table.unknown_reason} IS NULL)
			OR (${table.state} = 'outcome_unknown' AND ${table.project_json} IS NULL AND ${table.attachment} IS NULL AND ${table.rejection_reason} IS NULL AND ${table.unknown_reason} IN ('interrupted', 'verification_failed'))
			`,
		),
		check(
			"hosted_project_clone_approvals_update_time_check",
			sql`
				(${table.state} IN ('requested', 'reused') AND ${table.updated_at} = ${table.created_at})
				OR (${table.state} IN ('approved', 'denied') AND ${table.updated_at} = ${table.decided_at})
				OR (${table.state} = 'executing' AND ${table.updated_at} = ${table.execution_started_at})
				OR (${table.state} IN ('applied', 'attachment_conflict', 'rejected', 'outcome_unknown'))
			`,
		),
	],
);

/** Stores exact provider preparation, destination proof, execution result, and registration outside journal payloads. */
export const HostedProjectCloneArtifacts = sqliteTable(
	"hosted_project_clone_artifacts",
	{
		approval_id: text("approval_id")
			.primaryKey()
			.references(() => HostedProjectCloneApprovals.approval_id, { onDelete: "cascade" }),
		request_json: text("request_json").notNull(),
		preparation_json: text("preparation_json").notNull(),
		destination_proof_json: text("destination_proof_json").notNull(),
		clone_result_json: text("clone_result_json"),
		registered_project_json: text("registered_project_json"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		check(
			"hosted_project_clone_artifacts_registration_pair_check",
			sql`(${table.registered_project_json} IS NULL) OR (${table.clone_result_json} IS NOT NULL)`,
		),
	],
);

/** Exclusively reserves both an empty destination and provider-native hosted identity until settlement. */
export const HostedProjectCloneClaims = sqliteTable(
	"hosted_project_clone_claims",
	{
		approval_id: text("approval_id")
			.primaryKey()
			.references(() => HostedProjectCloneApprovals.approval_id, { onDelete: "cascade" }),
		thread_id: text("thread_id").notNull(),
		canonical_root: text("canonical_root").notNull(),
		provider_id: text("provider_id").notNull(),
		canonical_host: text("canonical_host").notNull(),
		native_id: text("native_id").notNull(),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull().default("unowned"),
		claimed_at: text("claimed_at").notNull(),
		lease_expires_at: text("lease_expires_at").notNull(),
		execution_started_at: text("execution_started_at"),
		execution_completed_at: text("execution_completed_at"),
	},
	(table) => [
		uniqueIndex("hosted_project_clone_claims_destination_unique").on(table.canonical_root),
		uniqueIndex("hosted_project_clone_claims_hosted_identity_unique").on(
			table.provider_id,
			table.canonical_host,
			table.native_id,
		),
		uniqueIndex("hosted_project_clone_claims_token_unique").on(table.claim_token),
		index("hosted_project_clone_claims_thread_index").on(table.thread_id),
		index("hosted_project_clone_claims_lease_index").on(table.lease_expires_at),
		check(
			"hosted_project_clone_claims_execution_pair_check",
			sql`${table.execution_completed_at} IS NULL OR ${table.execution_started_at} IS NOT NULL`,
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
		open_mode: text("open_mode").notNull().default("start"),
		native_thread_id: text("native_thread_id"),
		native_resume_json: text("native_resume_json"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("orchestration_runs_thread_id_index").on(table.thread_id),
		index("orchestration_runs_status_index").on(table.status),
		check("orchestration_runs_open_mode_check", sql`${table.open_mode} IN ('start', 'resume')`),
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
		observation_id: text("observation_id").notNull(),
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
		primaryKey({ columns: [table.run_id, table.observation_id] }),
		index("orchestration_raw_observations_run_sequence_index").on(table.run_id, table.sequence),
	],
);

export const RunUsageSamples = sqliteTable(
	"run_usage_samples",
	{
		run_id: text("run_id").notNull(),
		scope_key: text("scope_key").notNull(),
		sample_scope: text("sample_scope").notNull(),
		input_tokens: integer("input_tokens").notNull(),
		output_tokens: integer("output_tokens").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.run_id, table.scope_key] }),
		index("run_usage_samples_run_scope_index").on(table.run_id, table.sample_scope),
		check(
			"run_usage_samples_scope_check",
			sql`${table.sample_scope} IN ('turn_total', 'run_total')`,
		),
		check(
			"run_usage_samples_input_tokens_check",
			sql`${table.input_tokens} BETWEEN 0 AND ${sql.raw(String(Number.MAX_SAFE_INTEGER))}`,
		),
		check(
			"run_usage_samples_output_tokens_check",
			sql`${table.output_tokens} BETWEEN 0 AND ${sql.raw(String(Number.MAX_SAFE_INTEGER))}`,
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
		continuation_index: integer("continuation_index").notNull().default(0),
		continuation_text: text("continuation_text"),
		engine_id: text("engine_id").notNull(),
		open_mode: text("open_mode").notNull().default("start"),
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
		uniqueIndex("agent_runs_assignment_attempt_continuation_unique").on(
			table.assignment_id,
			table.attempt,
			table.continuation_index,
		),
		index("agent_runs_group_id_index").on(table.group_id),
		index("agent_runs_dispatch_status_index").on(table.dispatch_status),
		index("agent_runs_assignment_id_index").on(table.assignment_id),
		check("agent_runs_continuation_index_check", sql`${table.continuation_index} >= 0`),
		check("agent_runs_open_mode_check", sql`${table.open_mode} IN ('start', 'resume')`),
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

/** Stores compact, thread-independent local preview target projections. */
export const PreviewTargets = sqliteTable(
	"preview_targets",
	{
		target_id: text("target_id").notNull(),
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		generation_id: text("generation_id").notNull(),
		url: text("url").notNull(),
		source_kind: text("source_kind"),
		source_id: text("source_id"),
		state: text("state").notNull(),
		health_status: text("health_status"),
		health_checked_at_ms: integer("health_checked_at_ms"),
		health_latency_ms: integer("health_latency_ms"),
		health_message: text("health_message"),
		health_status_code: integer("health_status_code"),
		created_at_ms: integer("created_at_ms").notNull(),
		updated_at_ms: integer("updated_at_ms").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.project_id, table.workspace_id, table.target_id] }),
		check(
			"preview_targets_source_check",
			sql`COALESCE((${table.source_kind} IS NULL AND ${table.source_id} IS NULL) OR (${table.source_kind} IS NOT NULL AND ${table.source_kind} IN ('process', 'terminal') AND ${table.source_id} IS NOT NULL), 0)`,
		),
		check(
			"preview_targets_state_check",
			sql`${table.state} IN ('healthy', 'registered', 'stopped', 'unhealthy')`,
		),
		check(
			"preview_targets_health_check",
			sql`COALESCE((${table.health_status} IS NULL AND ${table.health_checked_at_ms} IS NULL AND ${table.health_latency_ms} IS NULL AND ${table.health_message} IS NULL AND ${table.health_status_code} IS NULL) OR (${table.health_status} IS NOT NULL AND ${table.health_status} IN ('healthy', 'unhealthy') AND ${table.health_checked_at_ms} IS NOT NULL AND ${table.health_latency_ms} IS NOT NULL AND ${table.health_latency_ms} >= 0), 0)`,
		),
	],
);

/** Fences one externally executing preview probe by durable command identity. */
export const PreviewTargetProbeClaims = sqliteTable(
	"preview_target_probe_claims",
	{
		message_id: text("message_id").primaryKey(),
		command_json: text("command_json").notNull(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		target_id: text("target_id").notNull(),
		target_generation_id: text("target_generation_id").notNull(),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		lease_expires_at: text("lease_expires_at").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("preview_target_probe_claims_token_unique").on(table.claim_token),
		index("preview_target_probe_claims_thread_index").on(table.thread_id),
		index("preview_target_probe_claims_lease_index").on(table.lease_expires_at),
	],
);

/** Serializes one target removal against browser handoffs across backend processes. */
export const PreviewTargetRemovalClaims = sqliteTable(
	"preview_target_removal_claims",
	{
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		target_id: text("target_id").notNull(),
		target_generation_id: text("target_generation_id"),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		lease_expires_at_ms: integer("lease_expires_at_ms").notNull(),
		created_at_ms: integer("created_at_ms").notNull(),
		updated_at_ms: integer("updated_at_ms").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.project_id, table.workspace_id, table.target_id] }),
		uniqueIndex("preview_target_removal_claims_token_unique").on(table.claim_token),
		index("preview_target_removal_claims_lease_index").on(table.lease_expires_at_ms),
		check(
			"preview_target_removal_claims_timestamp_check",
			sql`${table.created_at_ms} >= 0 AND ${table.updated_at_ms} >= ${table.created_at_ms} AND ${table.lease_expires_at_ms} >= ${table.updated_at_ms}`,
		),
	],
);

/** Records one committed removal whose exact-generation inspection fence remains owed. */
export const PreviewTargetRemovalFences = sqliteTable(
	"preview_target_removal_fences",
	{
		message_id: text("message_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		target_id: text("target_id").notNull(),
		target_generation_id: text("target_generation_id").notNull(),
		committed_at_ms: integer("committed_at_ms").notNull(),
	},
	(table) => [
		uniqueIndex("preview_target_removal_fences_scope_unique").on(
			table.project_id,
			table.workspace_id,
			table.target_id,
		),
		index("preview_target_removal_fences_thread_index").on(table.thread_id),
		index("preview_target_removal_fences_generation_scope_index").on(
			table.project_id,
			table.workspace_id,
			table.target_id,
			table.target_generation_id,
		),
		check("preview_target_removal_fences_timestamp_check", sql`${table.committed_at_ms} >= 0`),
	],
);

/** Fences one non-idempotent external-browser handoff by durable command identity. */
export const PreviewBrowserLaunches = sqliteTable(
	"preview_browser_launches",
	{
		message_id: text("message_id").primaryKey(),
		command_json: text("command_json").notNull(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		target_id: text("target_id").notNull(),
		target_generation_id: text("target_generation_id").notNull(),
		url: text("url").notNull(),
		initiator_kind: text("initiator_kind").notNull(),
		initiator_agent_id: text("initiator_agent_id"),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		lease_expires_at_ms: integer("lease_expires_at_ms").notNull(),
		state: text("state").notNull(),
		reason: text("reason"),
		requested_at_ms: integer("requested_at_ms").notNull(),
		updated_at_ms: integer("updated_at_ms").notNull(),
	},
	(table) => [
		uniqueIndex("preview_browser_launches_claim_token_unique").on(table.claim_token),
		index("preview_browser_launches_lease_index").on(table.lease_expires_at_ms),
		index("preview_browser_launches_scope_index").on(
			table.project_id,
			table.workspace_id,
			table.updated_at_ms,
		),
		index("preview_browser_launches_thread_index").on(table.thread_id),
		index("preview_browser_launches_target_index").on(
			table.project_id,
			table.workspace_id,
			table.target_id,
		),
		check(
			"preview_browser_launches_initiator_check",
			sql`COALESCE((${table.initiator_kind} = 'user' AND ${table.initiator_agent_id} IS NULL) OR (${table.initiator_kind} = 'agent' AND ${table.initiator_agent_id} IS NOT NULL), 0)`,
		),
		check(
			"preview_browser_launches_state_check",
			sql`${table.state} IN ('accepted', 'dispatching', 'dispatched', 'outcome_unknown', 'rejected')`,
		),
		check(
			"preview_browser_launches_reason_check",
			sql`COALESCE((${table.state} IN ('accepted', 'dispatching', 'dispatched') AND ${table.reason} IS NULL) OR (${table.state} = 'outcome_unknown' AND ${table.reason} IN ('interrupted', 'launcher_failed')) OR (${table.state} = 'rejected' AND ${table.reason} IN ('launcher_rejected', 'launcher_unavailable', 'target_changed')), 0)`,
		),
		check(
			"preview_browser_launches_timestamp_check",
			sql`${table.requested_at_ms} >= 0 AND ${table.updated_at_ms} >= ${table.requested_at_ms}`,
		),
	],
);

/** Stores source-safe state for one explicit external-browser inspection attachment. */
export const PreviewInspectionSessions = sqliteTable(
	"preview_inspection_sessions",
	{
		inspection_id: text("inspection_id").primaryKey(),
		attach_message_id: text("attach_message_id").notNull(),
		attach_command_json: text("attach_command_json").notNull(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		target_id: text("target_id").notNull(),
		target_generation_id: text("target_generation_id").notNull(),
		url: text("url").notNull(),
		connector_id: text("connector_id").notNull(),
		initiator_kind: text("initiator_kind").notNull(),
		initiator_agent_id: text("initiator_agent_id"),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		lease_expires_at_ms: integer("lease_expires_at_ms").notNull(),
		state: text("state").notNull(),
		reason: text("reason"),
		requested_at_ms: integer("requested_at_ms").notNull(),
		updated_at_ms: integer("updated_at_ms").notNull(),
	},
	(table) => [
		uniqueIndex("preview_inspection_sessions_attach_message_unique").on(
			table.attach_message_id,
		),
		uniqueIndex("preview_inspection_sessions_claim_token_unique").on(table.claim_token),
		index("preview_inspection_sessions_lease_index").on(table.lease_expires_at_ms),
		index("preview_inspection_sessions_scope_index").on(
			table.project_id,
			table.workspace_id,
			table.updated_at_ms,
		),
		index("preview_inspection_sessions_thread_index").on(table.thread_id),
		index("preview_inspection_sessions_target_index").on(
			table.project_id,
			table.workspace_id,
			table.target_id,
		),
		check(
			"preview_inspection_sessions_initiator_check",
			sql`COALESCE((${table.initiator_kind} = 'user' AND ${table.initiator_agent_id} IS NULL) OR (${table.initiator_kind} = 'agent' AND ${table.initiator_agent_id} IS NOT NULL), 0)`,
		),
		check(
			"preview_inspection_sessions_state_check",
			sql`${table.state} IN ('attached', 'attaching', 'disconnected', 'failed')`,
		),
		check(
			"preview_inspection_sessions_reason_check",
			sql`COALESCE((${table.state} IN ('attached', 'attaching') AND ${table.reason} IS NULL) OR (${table.state} = 'failed' AND ${table.reason} IN ('connector_rejected', 'connector_unavailable', 'target_changed')) OR (${table.state} = 'disconnected' AND ${table.reason} IN ('connection_lost', 'detached', 'interrupted', 'target_changed', 'thread_erased')), 0)`,
		),
		check(
			"preview_inspection_sessions_timestamp_check",
			sql`${table.requested_at_ms} >= 0 AND ${table.updated_at_ms} >= ${table.requested_at_ms}`,
		),
	],
);

/** Stores one canonical checkout root for each durable hosted project. */
export const Projects = sqliteTable(
	"projects",
	{
		project_id: text("project_id").primaryKey(),
		workspace_id: text("workspace_id").notNull(),
		canonical_root: text("canonical_root").notNull(),
		display_name: text("display_name").notNull(),
		registered_at: text("registered_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("projects_workspace_id_unique").on(table.workspace_id),
		uniqueIndex("projects_canonical_root_unique").on(table.canonical_root),
		index("projects_registered_at_index").on(table.registered_at),
	],
);

/** Binds exactly one canonical hosted origin to each registered project. */
export const ProjectHostedOrigins = sqliteTable(
	"project_hosted_origins",
	{
		project_id: text("project_id")
			.primaryKey()
			.references(() => Projects.project_id, { onDelete: "cascade" }),
		provider_id: text("provider_id").notNull(),
		canonical_host: text("canonical_host").notNull(),
		owner: text("owner").notNull(),
		name: text("name").notNull(),
		native_id: text("native_id").notNull(),
		selected_account_login: text("selected_account_login").notNull(),
		clone_url: text("clone_url").notNull(),
		web_url: text("web_url").notNull(),
		remote_name: text("remote_name").notNull(),
		fetch_url: text("fetch_url").notNull(),
		push_url: text("push_url").notNull(),
	},
	(table) => [
		uniqueIndex("project_hosted_origins_native_identity_unique").on(
			table.provider_id,
			table.canonical_host,
			table.native_id,
		),
		uniqueIndex("project_hosted_origins_coordinate_unique").on(
			table.provider_id,
			table.canonical_host,
			table.owner,
			table.name,
		),
		index("project_hosted_origins_project_id_index").on(table.project_id),
	],
);

/** Stores the latest canonical hosted review and CI projection for one project. */
export const HostedGitSnapshots = sqliteTable(
	"hosted_git_snapshots",
	{
		project_id: text("project_id")
			.primaryKey()
			.references(() => Projects.project_id, { onDelete: "cascade" }),
		lookup_json: text("lookup_json").notNull(),
		observed_at: text("observed_at").notNull(),
		version: integer("version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
	},
	(table) => [
		index("hosted_git_snapshots_journal_sequence_index").on(table.journal_sequence),
		check("hosted_git_snapshots_version_check", sql`${table.version} >= 1`),
		check("hosted_git_snapshots_journal_sequence_check", sql`${table.journal_sequence} >= 1`),
	],
);

/** Binds one exact refresh command to the hosted snapshot version and event it produced. */
export const HostedGitSnapshotOperations = sqliteTable(
	"hosted_git_snapshot_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id")
			.notNull()
			.references(() => Projects.project_id, { onDelete: "cascade" }),
		workspace_id: text("workspace_id").notNull(),
		snapshot_version: integer("snapshot_version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		sent_at: text("sent_at").notNull(),
	},
	(table) => [
		uniqueIndex("hosted_git_snapshot_operations_source_command_unique").on(
			table.source_command_id,
		),
		index("hosted_git_snapshot_operations_thread_index").on(table.thread_id),
		index("hosted_git_snapshot_operations_project_index").on(table.project_id),
		check(
			"hosted_git_snapshot_operations_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check("hosted_git_snapshot_operations_version_check", sql`${table.snapshot_version} >= 1`),
		check(
			"hosted_git_snapshot_operations_journal_sequence_check",
			sql`${table.journal_sequence} >= 1`,
		),
	],
);

/** Stores public, source-safe approval state for hosted Git provider writes. */
export const HostedGitMutationApprovals = sqliteTable(
	"hosted_git_mutation_approvals",
	{
		approval_id: text("approval_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		thread_id: text("thread_id").notNull(),
		workspace_id: text("workspace_id").notNull(),
		snapshot_version: integer("snapshot_version").notNull(),
		expected_head_commit: text("expected_head_commit").notNull(),
		pull_request_number: integer("pull_request_number").notNull(),
		pull_request_origin_json: text("pull_request_origin_json").notNull(),
		repository_json: text("repository_json").notNull(),
		selection_json: text("selection_json").notNull(),
		operation_summary_json: text("operation_summary_json").notNull(),
		state: text("state").notNull(),
		decision_message_id: text("decision_message_id"),
		approved: integer("approved", { mode: "boolean" }),
		decided_at: text("decided_at"),
		execution_started_at: text("execution_started_at"),
		result_json: text("result_json"),
		rejection_reason: text("rejection_reason"),
		unknown_reason: text("unknown_reason"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("hosted_git_mutation_approvals_source_command_unique").on(
			table.source_command_id,
		),
		uniqueIndex("hosted_git_mutation_approvals_decision_message_unique").on(
			table.decision_message_id,
		),
		index("hosted_git_mutation_approvals_thread_index").on(table.thread_id),
		index("hosted_git_mutation_approvals_workspace_index").on(table.workspace_id),
		index("hosted_git_mutation_approvals_state_index").on(table.state),
		check(
			"hosted_git_mutation_approvals_fingerprint_check",
			sql`
				length(${table.request_fingerprint}) = 64
				AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"hosted_git_mutation_approvals_target_check",
			sql`${table.snapshot_version} >= 1 AND ${table.pull_request_number} >= 1`,
		),
		check(
			"hosted_git_mutation_approvals_state_check",
			sql`
				${table.state} IN (
					'requested', 'approved', 'executing', 'applied',
					'rejected', 'outcome_unknown', 'denied'
				)
			`,
		),
		check(
			"hosted_git_mutation_approvals_decision_check",
			sql`
				(
					${table.state} = 'requested'
					AND ${table.decision_message_id} IS NULL
					AND ${table.approved} IS NULL
					AND ${table.decided_at} IS NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'denied'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 0
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'approved'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NULL
				)
				OR (
					${table.state} = 'executing'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
				)
				OR (
					${table.state} = 'rejected'
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
				)
				OR (
					${table.state} IN ('applied', 'outcome_unknown')
					AND ${table.decision_message_id} IS NOT NULL
					AND ${table.approved} = 1
					AND ${table.decided_at} IS NOT NULL
					AND ${table.execution_started_at} IS NOT NULL
				)
			`,
		),
		check(
			"hosted_git_mutation_approvals_outcome_check",
			sql`
				(
					${table.state} IN ('requested', 'approved', 'executing', 'denied')
					AND ${table.result_json} IS NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'applied'
					AND ${table.result_json} IS NOT NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'rejected'
					AND ${table.result_json} IS NULL
					AND ${table.rejection_reason} IS NOT NULL
					AND ${table.unknown_reason} IS NULL
				)
				OR (
					${table.state} = 'outcome_unknown'
					AND ${table.result_json} IS NULL
					AND ${table.rejection_reason} IS NULL
					AND ${table.unknown_reason} IS NOT NULL
				)
			`,
		),
	],
);

/** Stores the exact private provider operation until terminal settlement scrubs it. */
export const HostedGitMutationArtifacts = sqliteTable(
	"hosted_git_mutation_artifacts",
	{
		approval_id: text("approval_id")
			.primaryKey()
			.references(() => HostedGitMutationApprovals.approval_id, {
				onDelete: "cascade",
			}),
		operation_json: text("operation_json"),
		operation_binding: text("operation_binding").notNull(),
		provider_result_json: text("provider_result_json"),
		selection_json: text("selection_json"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		check(
			"hosted_git_mutation_artifacts_binding_check",
			sql`
				length(${table.operation_binding}) = 64
				AND ${table.operation_binding} NOT GLOB '*[^0-9a-f]*'
			`,
		),
		check(
			"hosted_git_mutation_artifacts_private_pair_check",
			sql`
				(
					${table.operation_json} IS NULL
					AND ${table.selection_json} IS NULL
				)
				OR (
					${table.operation_json} IS NOT NULL
					AND ${table.selection_json} IS NOT NULL
				)
			`,
		),
	],
);

/** Serializes one hosted provider write per visible workspace across runtimes. */
export const HostedGitMutationClaims = sqliteTable(
	"hosted_git_mutation_claims",
	{
		workspace_id: text("workspace_id").primaryKey(),
		approval_id: text("approval_id")
			.notNull()
			.references(() => HostedGitMutationApprovals.approval_id, {
				onDelete: "cascade",
			}),
		thread_id: text("thread_id").notNull(),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull().default("unowned"),
		claimed_at: text("claimed_at").notNull(),
		lease_expires_at: text("lease_expires_at").notNull(),
		execution_started_at: text("execution_started_at"),
		execution_completed_at: text("execution_completed_at"),
	},
	(table) => [
		uniqueIndex("hosted_git_mutation_claims_approval_unique").on(table.approval_id),
		uniqueIndex("hosted_git_mutation_claims_token_unique").on(table.claim_token),
		index("hosted_git_mutation_claims_thread_index").on(table.thread_id),
		index("hosted_git_mutation_claims_lease_index").on(table.lease_expires_at),
		check(
			"hosted_git_mutation_claims_execution_pair_check",
			sql`${table.execution_completed_at} IS NULL OR ${table.execution_started_at} IS NOT NULL`,
		),
	],
);

/** Stores private baselines alongside the source-free projection of one external wait. */
export const ExternalWaits = sqliteTable(
	"external_waits",
	{
		wait_id: text("wait_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		project_id: text("project_id")
			.notNull()
			.references(() => Projects.project_id, { onDelete: "restrict" }),
		workspace_id: text("workspace_id").notNull(),
		source_run_id: text("source_run_id").notNull(),
		owner_json: text("owner_json").notNull(),
		target_json: text("target_json").notNull(),
		gates_json: text("gates_json").notNull(),
		baseline_json: text("baseline_json").notNull(),
		baseline_fingerprint: text("baseline_fingerprint").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		state: text("state").notNull(),
		state_json: text("state_json").notNull(),
		generation: integer("generation").notNull().default(1),
		maximum_generation: integer("maximum_generation").notNull().default(3),
		next_observation_at: text("next_observation_at").notNull(),
		timeout_at: text("timeout_at").notNull(),
		observer_lease_owner: text("observer_lease_owner"),
		observer_lease_expires_at: text("observer_lease_expires_at"),
		source_closed_at: text("source_closed_at"),
		version: integer("version").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("external_waits_source_run_unique").on(table.source_run_id),
		index("external_waits_thread_index").on(table.thread_id, table.updated_at, table.wait_id),
		index("external_waits_observation_index").on(
			table.state,
			table.next_observation_at,
			table.observer_lease_expires_at,
		),
		check(
			"external_waits_fingerprint_check",
			sql`
			length(${table.baseline_fingerprint}) = 64
			AND ${table.baseline_fingerprint} NOT GLOB '*[^0-9a-f]*'
			AND length(${table.request_fingerprint}) = 64
			AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'
		`,
		),
		check(
			"external_waits_generation_check",
			sql`
			${table.generation} >= 1
			AND ${table.maximum_generation} >= ${table.generation}
			AND ${table.maximum_generation} <= 10
		`,
		),
		check(
			"external_waits_version_sequence_check",
			sql`${table.version} >= 1 AND ${table.journal_sequence} >= 1`,
		),
		check(
			"external_waits_state_check",
			sql`${table.state} IN ('waiting', 'wake_pending', 'woken', 'suspended', 'cancelled', 'exhausted')`,
		),
		check(
			"external_waits_observer_lease_check",
			sql`
			(
				${table.observer_lease_owner} IS NULL
				AND ${table.observer_lease_expires_at} IS NULL
			)
			OR (
				${table.observer_lease_owner} IS NOT NULL
				AND ${table.observer_lease_expires_at} IS NOT NULL
			)
		`,
		),
	],
);

/** Binds exact external-wait source commands to their durable public result. */
export const ExternalWaitOperations = sqliteTable(
	"external_wait_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		source_command_id: text("source_command_id").notNull(),
		kind: text("kind").notNull(),
		wait_id: text("wait_id")
			.notNull()
			.references(() => ExternalWaits.wait_id, { onDelete: "cascade" }),
		thread_id: text("thread_id").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		sent_at: text("sent_at").notNull(),
		result_snapshot_json: text("result_snapshot_json").notNull(),
		journal_sequence: integer("journal_sequence").notNull(),
	},
	(table) => [
		uniqueIndex("external_wait_operations_source_command_unique").on(table.source_command_id),
		index("external_wait_operations_wait_index").on(table.wait_id),
		check(
			"external_wait_operations_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"external_wait_operations_kind_check",
			sql`${table.kind} IN ('request', 'cancel', 'manual_resume')`,
		),
	],
);

/** Queues exactly one deterministic follow-up command for each wake trigger. */
export const ExternalWaitWakeOutbox = sqliteTable(
	"external_wait_wake_outbox",
	{
		outbox_id: text("outbox_id").primaryKey(),
		wait_id: text("wait_id")
			.notNull()
			.references(() => ExternalWaits.wait_id, { onDelete: "cascade" }),
		trigger_fingerprint: text("trigger_fingerprint").notNull(),
		follow_up_command_id: text("follow_up_command_id").notNull(),
		follow_up_run_id: text("follow_up_run_id").notNull(),
		mode: text("mode"),
		state: text("state").notNull(),
		lease_owner: text("lease_owner"),
		lease_expires_at: text("lease_expires_at"),
		trigger_json: text("trigger_json").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("external_wait_wake_outbox_wait_unique").on(table.wait_id),
		uniqueIndex("external_wait_wake_outbox_command_unique").on(table.follow_up_command_id),
		uniqueIndex("external_wait_wake_outbox_run_unique").on(table.follow_up_run_id),
		index("external_wait_wake_outbox_state_index").on(table.state, table.lease_expires_at),
		check(
			"external_wait_wake_outbox_fingerprint_check",
			sql`length(${table.trigger_fingerprint}) = 64 AND ${table.trigger_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"external_wait_wake_outbox_state_check",
			sql`${table.state} IN ('pending', 'claimed', 'settled', 'cancelled')`,
		),
		check(
			"external_wait_wake_outbox_mode_check",
			sql`
			(
				${table.state} = 'pending'
				AND ${table.mode} IS NULL
				AND ${table.lease_owner} IS NULL
				AND ${table.lease_expires_at} IS NULL
			)
			OR (
				${table.state} = 'claimed'
				AND ${table.mode} IS NULL
				AND ${table.lease_owner} IS NOT NULL
				AND ${table.lease_expires_at} IS NOT NULL
			)
			OR (
				${table.state} = 'settled'
				AND ${table.mode} IN ('native_resume', 'linked_run')
				AND ${table.lease_owner} IS NULL
				AND ${table.lease_expires_at} IS NULL
			)
			OR (
				${table.state} = 'cancelled'
				AND ${table.mode} IS NULL
				AND ${table.lease_owner} IS NULL
				AND ${table.lease_expires_at} IS NULL
			)
		`,
		),
	],
);

/** Stores source-safe lifecycle state and immutable descriptor snapshots for one tool invocation. */
export const ToolInvocations = sqliteTable(
	"tool_invocations",
	{
		invocation_id: text("invocation_id").primaryKey(),
		request_id: text("request_id").notNull(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		agent_id: text("agent_id").notNull(),
		workspace_id: text("workspace_id"),
		owner_kind: text("owner_kind").notNull(),
		tool_id: text("tool_id").notNull(),
		revision: integer("revision").notNull(),
		source: text("source").notNull(),
		effect: text("effect").notNull(),
		approval_policy: text("approval_policy").notNull(),
		label: text("label").notNull(),
		summary: text("summary").notNull(),
		input_schema_json: text("input_schema_json").notNull(),
		descriptor_fingerprint: text("descriptor_fingerprint").notNull(),
		recovery_policy: text("recovery_policy").notNull(),
		approval_id: text("approval_id"),
		decision_id: text("decision_id"),
		decision: text("decision"),
		state: text("state").notNull(),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
		decided_at: text("decided_at"),
		started_at: text("started_at"),
		suspended_at: text("suspended_at"),
		settled_at: text("settled_at"),
		current_journal_sequence: integer("current_journal_sequence").notNull(),
	},
	(table) => [
		uniqueIndex("tool_invocations_request_unique").on(table.request_id),
		uniqueIndex("tool_invocations_approval_unique").on(table.approval_id),
		uniqueIndex("tool_invocations_decision_unique").on(table.decision_id),
		uniqueIndex("tool_invocations_invocation_approval_unique").on(
			table.invocation_id,
			table.approval_id,
		),
		index("tool_invocations_thread_index").on(table.thread_id),
		check(
			"tool_invocations_owner_kind_check",
			sql`${table.owner_kind} IN ('ordinary_run', 'graph_run')`,
		),
		check("tool_invocations_revision_check", sql`${table.revision} > 0`),
		check("tool_invocations_source_check", sql`${table.source} IN ('artisan', 'marketplace')`),
		check(
			"tool_invocations_effect_check",
			sql`${table.effect} IN ('read', 'durable_state', 'workspace_mutation', 'unknown')`,
		),
		check(
			"tool_invocations_descriptor_fingerprint_check",
			sql`length(${table.descriptor_fingerprint}) = 64 AND ${table.descriptor_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"tool_invocations_input_schema_size_check",
			sql`json_valid(${table.input_schema_json}) = 1 AND length(CAST(${table.input_schema_json} AS BLOB)) <= ${sql.raw(String(tool_json_maximum_bytes))}`,
		),
		check(
			"tool_invocations_recovery_policy_check",
			sql`${table.recovery_policy} IN ('retry', 'outcome_unknown')`,
		),
		check(
			"tool_invocations_lifecycle_check",
			sql`
				(
					${table.approval_policy} = 'automatic'
					AND ${table.approval_id} IS NULL
					AND ${table.decision_id} IS NULL
					AND ${table.decision} IS NULL
					AND ${table.decided_at} IS NULL
					AND (
						(${table.state} = 'pending' AND ${table.started_at} IS NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NULL)
						OR (${table.state} = 'running' AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NULL)
						OR (${table.state} IN ('completed', 'failed', 'outcome_unknown') AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NOT NULL)
						OR (${table.state} = 'suspended' AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NOT NULL AND ${table.settled_at} IS NULL)
					)
				)
				OR (
					${table.approval_policy} = 'required'
					AND ${table.approval_id} IS NOT NULL
					AND (
						(${table.state} = 'approval_required' AND ${table.decision_id} IS NULL AND ${table.decision} IS NULL AND ${table.decided_at} IS NULL AND ${table.started_at} IS NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NULL)
						OR (${table.state} = 'denied' AND ${table.decision_id} IS NOT NULL AND ${table.decision} = 'denied' AND ${table.decided_at} IS NOT NULL AND ${table.started_at} IS NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NOT NULL)
						OR (${table.state} = 'pending' AND ${table.decision_id} IS NOT NULL AND ${table.decision} = 'approved' AND ${table.decided_at} IS NOT NULL AND ${table.started_at} IS NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NULL)
						OR (${table.state} = 'running' AND ${table.decision_id} IS NOT NULL AND ${table.decision} = 'approved' AND ${table.decided_at} IS NOT NULL AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NULL)
						OR (${table.state} IN ('completed', 'failed', 'outcome_unknown') AND ${table.decision_id} IS NOT NULL AND ${table.decision} = 'approved' AND ${table.decided_at} IS NOT NULL AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NULL AND ${table.settled_at} IS NOT NULL)
						OR (${table.state} = 'suspended' AND ${table.decision_id} IS NOT NULL AND ${table.decision} = 'approved' AND ${table.decided_at} IS NOT NULL AND ${table.started_at} IS NOT NULL AND ${table.suspended_at} IS NOT NULL AND ${table.settled_at} IS NULL)
					)
				)
			`,
		),
		check(
			"tool_invocations_timestamp_format_check",
			sql`
				strftime('%Y-%m-%dT%H:%M:%fZ', ${table.created_at}) IS ${table.created_at}
				AND substr(${table.created_at}, 12, 2) BETWEEN '00' AND '23'
				AND strftime('%Y-%m-%dT%H:%M:%fZ', ${table.updated_at}) IS ${table.updated_at}
				AND substr(${table.updated_at}, 12, 2) BETWEEN '00' AND '23'
				AND (${table.decided_at} IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', ${table.decided_at}) IS ${table.decided_at} AND substr(${table.decided_at}, 12, 2) BETWEEN '00' AND '23'))
				AND (${table.started_at} IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', ${table.started_at}) IS ${table.started_at} AND substr(${table.started_at}, 12, 2) BETWEEN '00' AND '23'))
				AND (${table.suspended_at} IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', ${table.suspended_at}) IS ${table.suspended_at} AND substr(${table.suspended_at}, 12, 2) BETWEEN '00' AND '23'))
				AND (${table.settled_at} IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', ${table.settled_at}) IS ${table.settled_at} AND substr(${table.settled_at}, 12, 2) BETWEEN '00' AND '23'))
			`,
		),
		check(
			"tool_invocations_timestamp_order_check",
			sql`
				${table.created_at} <= ${table.updated_at}
				AND (${table.decided_at} IS NULL OR (${table.decided_at} >= ${table.created_at} AND ${table.decided_at} <= ${table.updated_at}))
				AND (${table.started_at} IS NULL OR (${table.started_at} >= ${table.created_at} AND ${table.started_at} <= ${table.updated_at}))
				AND (${table.suspended_at} IS NULL OR (${table.suspended_at} >= ${table.created_at} AND ${table.suspended_at} <= ${table.updated_at}))
				AND (${table.settled_at} IS NULL OR (${table.settled_at} >= ${table.created_at} AND ${table.settled_at} <= ${table.updated_at}))
				AND (${table.decided_at} IS NULL OR ${table.started_at} IS NULL OR ${table.decided_at} <= ${table.started_at})
				AND (${table.started_at} IS NULL OR ${table.suspended_at} IS NULL OR ${table.started_at} <= ${table.suspended_at})
				AND (${table.started_at} IS NULL OR ${table.settled_at} IS NULL OR ${table.started_at} <= ${table.settled_at})
				AND (${table.decided_at} IS NULL OR ${table.settled_at} IS NULL OR ${table.decided_at} <= ${table.settled_at})
			`,
		),
		check(
			"tool_invocations_journal_sequence_check",
			sql`${table.current_journal_sequence} > 0`,
		),
	],
);

/** Records exact-replay tool invocation and approval commands without private payloads. */
export const ToolControlCommands = sqliteTable(
	"tool_control_commands",
	{
		command_id: text("command_id").primaryKey(),
		kind: text("kind").notNull(),
		invocation_id: text("invocation_id")
			.notNull()
			.references(() => ToolInvocations.invocation_id, { onDelete: "cascade" }),
		approval_id: text("approval_id"),
		decision: text("decision"),
		request_fingerprint: text("request_fingerprint").notNull(),
		accepted_at: text("accepted_at").notNull(),
	},
	(table) => [
		index("tool_control_commands_invocation_index").on(table.invocation_id),
		foreignKey({
			name: "tool_control_commands_invocation_approval_fk",
			columns: [table.invocation_id, table.approval_id],
			foreignColumns: [ToolInvocations.invocation_id, ToolInvocations.approval_id],
		}).onDelete("cascade"),
		check(
			"tool_control_commands_kind_check",
			sql`
				(${table.kind} = 'invoke' AND ${table.approval_id} IS NULL AND ${table.decision} IS NULL)
				OR (${table.kind} = 'decision' AND ${table.approval_id} IS NOT NULL AND ${table.decision} IN ('approved', 'denied'))
			`,
		),
		check(
			"tool_control_commands_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"tool_control_commands_accepted_at_check",
			sql`strftime('%Y-%m-%dT%H:%M:%fZ', ${table.accepted_at}) IS ${table.accepted_at} AND substr(${table.accepted_at}, 12, 2) BETWEEN '00' AND '23'`,
		),
	],
);

/** Stores private invocation arguments and, after completion, the paired private result. */
export const ToolInvocationPrivate = sqliteTable(
	"tool_invocation_private",
	{
		invocation_id: text("invocation_id")
			.primaryKey()
			.references(() => ToolInvocations.invocation_id, { onDelete: "cascade" }),
		request_fingerprint: text("request_fingerprint").notNull(),
		arguments_json: text("arguments_json").notNull(),
		arguments_digest: text("arguments_digest").notNull(),
		result_json: text("result_json"),
		result_digest: text("result_digest"),
	},
	(table) => [
		check(
			"tool_invocation_private_request_fingerprint_check",
			sql`length(${table.request_fingerprint}) = 64 AND ${table.request_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"tool_invocation_private_arguments_digest_check",
			sql`length(${table.arguments_digest}) = 64 AND ${table.arguments_digest} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"tool_invocation_private_arguments_size_check",
			sql`json_valid(${table.arguments_json}) = 1 AND length(CAST(${table.arguments_json} AS BLOB)) <= ${sql.raw(String(tool_json_maximum_bytes))}`,
		),
		check(
			"tool_invocation_private_result_size_check",
			sql`${table.result_json} IS NULL OR (json_valid(${table.result_json}) = 1 AND length(CAST(${table.result_json} AS BLOB)) <= ${sql.raw(String(tool_json_maximum_bytes))})`,
		),
		check(
			"tool_invocation_private_result_pair_check",
			sql`(${table.result_json} IS NULL AND ${table.result_digest} IS NULL) OR (${table.result_json} IS NOT NULL AND ${table.result_digest} IS NOT NULL AND length(${table.result_digest}) = 64 AND ${table.result_digest} NOT GLOB '*[^0-9a-f]*')`,
		),
	],
);

/** Leases one tool invocation to an executing backend instance. */
export const ToolExecutionClaims = sqliteTable(
	"tool_execution_claims",
	{
		invocation_id: text("invocation_id")
			.primaryKey()
			.references(() => ToolInvocations.invocation_id, { onDelete: "cascade" }),
		claim_token: text("claim_token").notNull(),
		owner_instance_id: text("owner_instance_id").notNull(),
		claimed_at: text("claimed_at").notNull(),
		lease_expires_at: text("lease_expires_at").notNull(),
		launch_started_at: text("launch_started_at"),
	},
	(table) => [
		uniqueIndex("tool_execution_claims_token_unique").on(table.claim_token),
		index("tool_execution_claims_lease_index").on(table.lease_expires_at),
		check(
			"tool_execution_claims_lease_time_check",
			sql`${table.lease_expires_at} >= ${table.claimed_at}`,
		),
		check(
			"tool_execution_claims_launch_time_check",
			sql`${table.launch_started_at} IS NULL OR (${table.launch_started_at} >= ${table.claimed_at} AND ${table.launch_started_at} <= ${table.lease_expires_at})`,
		),
		check(
			"tool_execution_claims_timestamp_format_check",
			sql`
				strftime('%Y-%m-%dT%H:%M:%fZ', ${table.claimed_at}) IS ${table.claimed_at}
				AND substr(${table.claimed_at}, 12, 2) BETWEEN '00' AND '23'
				AND strftime('%Y-%m-%dT%H:%M:%fZ', ${table.lease_expires_at}) IS ${table.lease_expires_at}
				AND substr(${table.lease_expires_at}, 12, 2) BETWEEN '00' AND '23'
				AND (${table.launch_started_at} IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', ${table.launch_started_at}) IS ${table.launch_started_at} AND substr(${table.launch_started_at}, 12, 2) BETWEEN '00' AND '23'))
			`,
		),
	],
);

/** Stores immutable canonical surface generations built from one fixed journal snapshot. */
export const SurfaceProjectionGenerations = sqliteTable(
	"surface_projection_generations",
	{
		generation_id: text("generation_id").primaryKey(),
		watermark: integer("watermark").notNull(),
		stream_cursors_json: text("stream_cursors_json").notNull(),
		item_count: integer("item_count").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		check("surface_projection_generations_watermark_check", sql`${table.watermark} >= 0`),
		check("surface_projection_generations_item_count_check", sql`${table.item_count} >= 0`),
		check(
			"surface_projection_generations_cursors_check",
			sql`json_valid(${table.stream_cursors_json}) = 1 AND json_type(${table.stream_cursors_json}) = 'array'`,
		),
		check(
			"surface_projection_generations_created_at_check",
			sql`strftime('%Y-%m-%dT%H:%M:%fZ', ${table.created_at}) IS ${table.created_at} AND substr(${table.created_at}, 12, 2) BETWEEN '00' AND '23'`,
		),
	],
);

/** Stores source-safe surface items within one immutable projection generation. */
export const SurfaceProjectionItems = sqliteTable(
	"surface_projection_items",
	{
		generation_id: text("generation_id")
			.notNull()
			.references(() => SurfaceProjectionGenerations.generation_id, { onDelete: "cascade" }),
		surface_id: text("surface_id").notNull(),
		item_json: text("item_json").notNull(),
	},
	(table) => [
		primaryKey({ columns: [table.generation_id, table.surface_id] }),
		check(
			"surface_projection_items_json_check",
			sql`json_valid(${table.item_json}) = 1 AND json_type(${table.item_json}) = 'object' AND length(CAST(${table.item_json} AS BLOB)) <= 32768`,
		),
	],
);

/** Points readers at one complete surface generation through an atomic singleton swap. */
export const SurfaceProjectionState = sqliteTable(
	"surface_projection_state",
	{
		state_id: integer("state_id").primaryKey(),
		generation_id: text("generation_id")
			.notNull()
			.references(() => SurfaceProjectionGenerations.generation_id, {
				onDelete: "restrict",
			}),
	},
	(table) => [check("surface_projection_state_singleton_check", sql`${table.state_id} = 1`)],
);

/** Stores privacy-bounded exact-replay export-control decisions without country values. */
export const ExportControlAuditDecisions = sqliteTable(
	"export_control_audit_decisions",
	{
		decision_id: text("decision_id").primaryKey(),
		action: text("action").notNull(),
		intent_fingerprint: text("intent_fingerprint").notNull(),
		decision_json: text("decision_json").notNull(),
		record_json: text("record_json").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		check(
			"export_control_audit_action_check",
			sql`${table.action} IN ('account', 'billing', 'distribution', 'hosted_sync', 'marketplace_delivery', 'release', 'update')`,
		),
		check(
			"export_control_audit_fingerprint_check",
			sql`length(${table.intent_fingerprint}) = 64 AND ${table.intent_fingerprint} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"export_control_audit_decision_json_check",
			sql`json_valid(${table.decision_json}) = 1 AND json_type(${table.decision_json}) = 'object' AND length(CAST(${table.decision_json} AS BLOB)) <= 8192`,
		),
		check(
			"export_control_audit_record_json_check",
			sql`json_valid(${table.record_json}) = 1 AND json_type(${table.record_json}) = 'object' AND length(CAST(${table.record_json} AS BLOB)) <= 8192`,
		),
		check(
			"export_control_audit_created_at_check",
			sql`strftime('%Y-%m-%dT%H:%M:%fZ', ${table.created_at}) IS ${table.created_at} AND substr(${table.created_at}, 12, 2) BETWEEN '00' AND '23'`,
		),
	],
);
