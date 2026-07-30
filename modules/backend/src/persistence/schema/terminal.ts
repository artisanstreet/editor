import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

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
