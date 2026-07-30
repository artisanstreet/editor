import {
	index,
	integer,
	primaryKey,
	sqliteTable,
	text,
	uniqueIndex,
} from "drizzle-orm/sqlite-core";

export const MarketplaceRoutines = sqliteTable(
	"marketplace_routines",
	{
		id: text("id").primaryKey(),
		display_name: text("display_name").notNull(),
		description: text("description").notNull(),
		instructions: text("instructions").notNull(),
		source_json: text("source_json").notNull(),
		version: text("version").notNull(),
		author: text("author"),
		scope_json: text("scope_json").notNull(),
		permissions_json: text("permissions_json").notNull(),
		compatibility_json: text("compatibility_json").notNull(),
		commands_json: text("commands_json").notNull(),
		files_json: text("files_json").notNull(),
		trust: text("trust").notNull(),
		enabled: integer("enabled", { mode: "boolean" }).notNull(),
		status: text("status").notNull(),
		artifact_refs_json: text("artifact_refs_json").notNull().default("[]"),
		removed_at: text("removed_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("marketplace_routines_scope_index").on(table.scope_json),
		index("marketplace_routines_enabled_index").on(table.enabled),
	],
);

/** Explicit approval-bound mutations and their canonical lifecycle ledger. */
export const MarketplaceRoutineOperations = sqliteTable(
	"marketplace_routine_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		kind: text("kind").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		approval_id: text("approval_id"),
		approval_fingerprint: text("approval_fingerprint"),
		approval_decision: text("approval_decision"),
		routine_id: text("routine_id"),
		preview_json: text("preview_json"),
		/** Exact reviewed canonical detail for approval recovery; never secret material. */
		detail_json: text("detail_json"),
		rollback_json: text("rollback_json"),
		/** A bounded durable ownership lease prevents concurrent provider effects. */
		dispatch_owner_id: text("dispatch_owner_id"),
		dispatch_lease_expires_at: text("dispatch_lease_expires_at"),
		state: text("state").notNull(),
		failure_code: text("failure_code"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_routine_operations_approval_unique").on(table.approval_id),
		index("marketplace_routine_operations_routine_index").on(table.routine_id),
	],
);

/** Per-engine native mirror state; runtime-only is durable and never a silent sync. */
export const MarketplaceRoutineMirrors = sqliteTable(
	"marketplace_routine_mirrors",
	{
		routine_id: text("routine_id").notNull(),
		engine_id: text("engine_id").notNull(),
		status: text("status").notNull(),
		observed_revision: text("observed_revision"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.routine_id, table.engine_id] })],
);

/** Canonical MCP capability records. Secrets are always opaque references held by the vault. */
export const MarketplaceCapabilities = sqliteTable(
	"marketplace_capabilities",
	{
		id: text("id").primaryKey(),
		display_name: text("display_name").notNull(),
		source_json: text("source_json").notNull(),
		transport_json: text("transport_json").notNull(),
		auth_json: text("auth_json").notNull(),
		scope_json: text("scope_json").notNull(),
		permissions_json: text("permissions_json").notNull(),
		compatibility_json: text("compatibility_json").notNull(),
		tools_json: text("tools_json").notNull(),
		resources_json: text("resources_json").notNull(),
		instructions: text("instructions"),
		policy_json: text("policy_json").notNull(),
		trust: text("trust").notNull(),
		enabled: integer("enabled", { mode: "boolean" }).notNull(),
		status: text("status").notNull(),
		lifecycle: text("lifecycle").notNull(),
		health_json: text("health_json").notNull(),
		raw_provider_metadata_json: text("raw_provider_metadata_json"),
		removed_at: text("removed_at"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("marketplace_capabilities_scope_index").on(table.scope_json),
		index("marketplace_capabilities_enabled_index").on(table.enabled),
		index("marketplace_capabilities_lifecycle_index").on(table.lifecycle),
	],
);

/** Every connection, approval, OAuth and invocation action is durable and idempotent. */
export const MarketplaceCapabilityOperations = sqliteTable(
	"marketplace_capability_operations",
	{
		operation_id: text("operation_id").primaryKey(),
		capability_id: text("capability_id").notNull(),
		kind: text("kind").notNull(),
		request_fingerprint: text("request_fingerprint").notNull(),
		approval_id: text("approval_id"),
		approval_fingerprint: text("approval_fingerprint"),
		approval_decision: text("approval_decision"),
		preview_json: text("preview_json"),
		/** Exact reviewed canonical detail for connect recovery; never secret material. */
		detail_json: text("detail_json"),
		state: text("state").notNull(),
		failure_code: text("failure_code"),
		artifact_id: text("artifact_id"),
		tool_name: text("tool_name"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_capability_operations_approval_unique").on(table.approval_id),
		index("marketplace_capability_operations_capability_index").on(table.capability_id),
	],
);

/** Bounded MCP tool results referenced by the invocation ledger; never embedded in events. */
export const MarketplaceCapabilityArtifacts = sqliteTable(
	"marketplace_capability_artifacts",
	{
		artifact_id: text("artifact_id").primaryKey(),
		capability_id: text("capability_id").notNull(),
		operation_id: text("operation_id").notNull(),
		tool_name: text("tool_name").notNull(),
		result_json: text("result_json").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("marketplace_capability_artifacts_operation_unique").on(table.operation_id),
		index("marketplace_capability_artifacts_capability_index").on(table.capability_id),
	],
);

/** Native provider config is a mirror, never an alternate canonical registry. */
export const MarketplaceCapabilityMirrors = sqliteTable(
	"marketplace_capability_mirrors",
	{
		capability_id: text("capability_id").notNull(),
		engine_id: text("engine_id").notNull(),
		status: text("status").notNull(),
		observed_revision: text("observed_revision"),
		last_error_code: text("last_error_code"),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [primaryKey({ columns: [table.capability_id, table.engine_id] })],
);

/** Stores idempotent, content-free workspace change operation lifecycles. */
