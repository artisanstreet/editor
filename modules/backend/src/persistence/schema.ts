import {
	index,
	integer,
	primaryKey,
	sqliteTable,
	text,
	uniqueIndex,
} from "drizzle-orm/sqlite-core";

export const JournalCommands = sqliteTable("journal_commands", {
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
});

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
		uniqueIndex("journal_events_stream_sequence_unique").on(
			table.stream_id,
			table.stream_sequence,
		),
		index("journal_events_correlation_id_index").on(table.correlation_id),
	],
);

export const Threads = sqliteTable("threads", {
	thread_id: text("thread_id").primaryKey(),
	title: text("title").notNull(),
	created_at: text("created_at").notNull(),
	updated_at: text("updated_at").notNull(),
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
