import { integer, primaryKey, sqliteTable, text } from "drizzle-orm/sqlite-core";

/**
 * Stores one row per starred catalog model. A row-per-favorite keeps the set
 * addressable by model id, so starring one model never rewrites the others.
 */
export const ModelFavorites = sqliteTable("model_favorites", {
	model_id: text("model_id").primaryKey(),
	favorited_at: text("favorited_at").notNull(),
});

/**
 * Stores the one shared row of defaults a new draft inherits. Permission lives
 * here rather than per model: it describes how much the agent may do in the
 * workspace, which does not change because the model did.
 */
export const SessionDefaults = sqliteTable("session_defaults", {
	agent_name_dataset: text("agent_name_dataset").notNull().default("norwegian"),
	auto_continue_usage_limits: integer("auto_continue_usage_limits", { mode: "boolean" })
		.notNull()
		.default(true),
	/**
	 * The protocol's `compaction_model` selection: `"inherited"` or a catalog
	 * model id. NULL means the curated per-harness default. The column keeps
	 * its original name to avoid a rename-only migration.
	 */
	compaction_model_id: text("compaction_model_id"),
	defaults_id: integer("defaults_id").primaryKey(),
	last_model_id: text("last_model_id"),
	onboarding_completed: integer("onboarding_completed", { mode: "boolean" })
		.notNull()
		.default(false),
	permission: text("permission").notNull(),
	thread_title_mode: text("thread_title_mode").notNull().default("summary"),
	updated_at: text("updated_at").notNull(),
});

/**
 * Stores the controls each catalog model was last configured with. A row per model
 * keeps effort, speed, and context window addressable by model id, because their
 * option sets differ and a shared value would be coerced onto models lacking it.
 */
export const SessionModelDefaults = sqliteTable("session_model_defaults", {
	context_window: text("context_window"),
	model_id: text("model_id").primaryKey(),
	reasoning_effort: text("reasoning_effort"),
	service_tier: text("service_tier"),
	updated_at: text("updated_at").notNull(),
});

/**
 * Stores one row per engine the user switched off. Row-per-engine like the
 * favorites table: disabling one engine never rewrites the others, and a
 * harness added to the catalog later arrives enabled by simple absence.
 */
export const DisabledEngines = sqliteTable("disabled_engines", {
	engine_id: text("engine_id").primaryKey(),
	disabled_at: text("disabled_at").notNull(),
});

/** Stores only content-free metadata for the one canonical global guidance file. */
export const GlobalGuidanceCanonical = sqliteTable("global_guidance_canonical", {
	canonical_id: integer("canonical_id").primaryKey(),
	content_hash: text("content_hash"),
	byte_count: integer("byte_count"),
	status: text("status").notNull(),
	selected_provider: text("selected_provider"),
	updated_at: text("updated_at").notNull(),
});

/** Stores metadata-only reconciliation state for each native provider mirror. */
export const GlobalGuidanceProviderSync = sqliteTable("global_guidance_provider_sync", {
	provider: text("provider").primaryKey(),
	status: text("status").notNull(),
	path: text("path"),
	modified_at: text("modified_at"),
	observed_hash: text("observed_hash"),
	observed_byte_count: integer("observed_byte_count"),
	applied_hash: text("applied_hash"),
	applied_byte_count: integer("applied_byte_count"),
	ignored_drift_hash: text("ignored_drift_hash"),
	backup_path: text("backup_path"),
	last_error_code: text("last_error_code"),
	updated_at: text("updated_at").notNull(),
});

/** Stores the one curated provider-neutral model behaviour setting. */
export const ModelBehaviourSettings = sqliteTable("model_behaviour_settings", {
	setting_id: text("setting_id").primaryKey(),
	value_json: text("value_json").notNull(),
	version: integer("version").notNull(),
	updated_at: text("updated_at").notNull(),
});

/** Stores content-free reconciliation metadata for each provider setting mapping. */
export const ModelBehaviourProviderStates = sqliteTable(
	"model_behaviour_provider_states",
	{
		provider_id: text("provider_id").notNull(),
		setting_id: text("setting_id").notNull(),
		status: text("status").notNull(),
		native_key: text("native_key"),
		target_path: text("target_path"),
		observed_hash: text("observed_hash"),
		applied_hash: text("applied_hash"),
		ignored_drift_hash: text("ignored_drift_hash"),
		backup_path: text("backup_path"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.provider_id, table.setting_id] })],
);

/** Canonical Routine records; provider formats remain import or mirror evidence. */
