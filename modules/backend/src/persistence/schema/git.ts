import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

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
