import { index, integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

/** Safe, ordered projection of one raw engine observation. */
export const SurfaceItems = sqliteTable(
	"surface_items",
	{
		projection_order: integer("projection_order").primaryKey({ autoIncrement: true }),
		surface_id: text("surface_id").notNull().unique(),
		observation_id: text("observation_id").notNull().unique(),
		thread_id: text("thread_id").notNull(),
		run_id: text("run_id").notNull(),
		group_id: text("group_id"),
		assignment_id: text("assignment_id"),
		sequence: integer("sequence").notNull(),
		category: text("category").notNull(),
		kind: text("kind").notNull(),
		summary_json: text("summary_json").notNull(),
		raw_origin_json: text("raw_origin_json"),
		occurred_at: text("occurred_at").notNull(),
	},
	(table) => [
		index("surface_items_thread_projection_order_index").on(
			table.thread_id,
			table.projection_order,
		),
		index("surface_items_thread_kind_projection_order_index").on(
			table.thread_id,
			table.kind,
			table.projection_order,
		),
		index("surface_items_thread_run_projection_order_index").on(
			table.thread_id,
			table.run_id,
			table.projection_order,
		),
		index("surface_items_thread_group_projection_order_index").on(
			table.thread_id,
			table.group_id,
			table.projection_order,
		),
	],
);

/**
 * Optional provider-neutral token totals; null means unavailable, never
 * zero-by-invention. `context_tokens` and `context_window_tokens` are the
 * latest reported context-window gauges rather than accumulating totals.
 */
export const SurfaceUsageTotals = sqliteTable(
	"surface_usage_totals",
	{
		run_id: text("run_id").primaryKey(),
		group_id: text("group_id"),
		assignment_id: text("assignment_id"),
		input_tokens: integer("input_tokens"),
		output_tokens: integer("output_tokens"),
		cached_input_tokens: integer("cached_input_tokens"),
		context_tokens: integer("context_tokens"),
		context_window_tokens: integer("context_window_tokens"),
		last_observation_id: text("last_observation_id").notNull().default(""),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("surface_usage_totals_assignment_id_index").on(table.assignment_id),
		index("surface_usage_totals_group_id_index").on(table.group_id),
	],
);
