import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

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

/** Durable cursors owned by consumers that have fully validated their initial replay. */
export const JournalConsumerCheckpoints = sqliteTable(
	"journal_consumer_checkpoints",
	{
		consumer_id: text("consumer_id").primaryKey(),
		journal_sequence: integer("journal_sequence").notNull(),
	},
	(table) => [
		check("journal_consumer_checkpoints_sequence_check", sql`${table.journal_sequence} >= 0`),
	],
);

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
		index("journal_events_type_sequence_index").on(table.event_type, table.sequence),
		index("journal_events_thread_sequence_index").on(table.thread_id, table.sequence),
		index("journal_events_thread_run_sequence_index").on(
			table.thread_id,
			table.run_id,
			table.sequence,
		),
		index("journal_events_thread_type_sequence_index").on(
			table.thread_id,
			table.event_type,
			table.sequence,
		),
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
		last_assistant_message: text("last_assistant_message"),
		rename_suggestion: text("rename_suggestion"),
		last_activity_at: text("last_activity_at").notNull().default("1970-01-01T00:00:00.000Z"),
		reader_activity_at: text("reader_activity_at")
			.notNull()
			.default("1970-01-01T00:00:00.000Z"),
		reader_acknowledged_activity_at: text("reader_acknowledged_activity_at")
			.notNull()
			.default("1970-01-01T00:00:00.000Z"),
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

/**
 * Remembers the workspace id minted for every project root ever located. Rows
 * are never deleted: detaching and re-attaching a project must yield the same
 * id, and historical thread evidence keeps resolving through it.
 */
export const ProjectIdentities = sqliteTable(
	"project_identities",
	{
		created_at: text("created_at").notNull(),
		project_id: text("project_id").notNull(),
		root_path: text("root_path").primaryKey(),
	},
	(table) => [uniqueIndex("project_identities_project_id_unique").on(table.project_id)],
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
