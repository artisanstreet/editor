import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

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
