import { sql } from "drizzle-orm";
import {
	blob,
	check,
	index,
	integer,
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
