import { integer, sqliteTable, text } from "drizzle-orm/sqlite-core";

export const ThreadRetentionPolicies = sqliteTable("thread_retention_policies", {
	policy_id: integer("policy_id").primaryKey(),
	enabled: integer("enabled", { mode: "boolean" }).notNull(),
	inactivity_days: integer("inactivity_days").notNull(),
	updated_at: text("updated_at").notNull(),
});

export const ThreadErasureClaims = sqliteTable("thread_erasure_claims", {
	thread_id: text("thread_id").primaryKey(),
	claimed_at: text("claimed_at").notNull(),
});

export const ThreadTombstones = sqliteTable("thread_tombstones", {
	thread_id: text("thread_id").primaryKey(),
	deleted_at: text("deleted_at").notNull(),
});
