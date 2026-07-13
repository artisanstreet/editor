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
			sql`${table.kind} IN ('refresh', 'checkout', 'recovery')`,
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
