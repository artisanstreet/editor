import { index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

export const ConversationThreads = sqliteTable("conversation_threads", {
	thread_id: text("thread_id").primaryKey(),
	next_ordinal: integer("next_ordinal").notNull().default(0),
	last_patch_sequence: integer("last_patch_sequence").notNull().default(0),
	journal_sequence: integer("journal_sequence").notNull().default(0),
	updated_at: text("updated_at").notNull(),
});

export const ConversationTurns = sqliteTable(
	"conversation_turns",
	{
		turn_id: text("turn_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		ordinal: integer("ordinal").notNull(),
		entity_json: text("entity_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_turns_thread_ordinal_unique").on(table.thread_id, table.ordinal),
		index("conversation_turns_thread_index").on(table.thread_id),
	],
);

export const ConversationItems = sqliteTable(
	"conversation_items",
	{
		item_id: text("item_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		turn_id: text("turn_id").notNull(),
		ordinal: integer("ordinal").notNull(),
		entity_json: text("entity_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_items_thread_ordinal_unique").on(table.thread_id, table.ordinal),
		index("conversation_items_thread_turn_index").on(table.thread_id, table.turn_id),
	],
);

export const ConversationPatches = sqliteTable(
	"conversation_patches",
	{
		patch_id: text("patch_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		sequence: integer("sequence").notNull(),
		patch_json: text("patch_json").notNull(),
	},
	(table) => [
		uniqueIndex("conversation_patches_thread_sequence_unique").on(
			table.thread_id,
			table.sequence,
		),
	],
);

/** One successful source admission means an exact replay cannot allocate another ordinal or patch. */
export const ConversationSources = sqliteTable("conversation_sources", {
	source_id: text("source_id").primaryKey(),
	thread_id: text("thread_id").notNull(),
	journal_sequence: integer("journal_sequence"),
	observed_at: text("observed_at").notNull(),
});
