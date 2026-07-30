import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

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
