import { index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

export const JournalCommands = sqliteTable("journal_commands", {
	message_id: text("message_id").primaryKey(),
	schema_version: integer("schema_version").notNull(),
	thread_id: text("thread_id").notNull(),
	run_id: text("run_id"),
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
