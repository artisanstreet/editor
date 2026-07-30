import { sql } from "drizzle-orm";
import { check, index, integer, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

/**
 * Stores private provider-neutral continuation metadata without widening the
 * provider-owned resume token persisted on the orchestration run.
 */
export const ThreadRunContinuationState = sqliteTable(
	"thread_run_continuation_state",
	{
		run_id: text("run_id").primaryKey(),
		thread_id: text("thread_id").notNull(),
		engine_id: text("engine_id").notNull(),
		model_id: text("model_id"),
		last_native_turn_id: text("last_native_turn_id"),
		last_observation_sequence: integer("last_observation_sequence").notNull().default(0),
		native_compaction_boundary_journal_sequence: integer(
			"native_compaction_boundary_journal_sequence",
		),
		native_compaction_observation_id: text("native_compaction_observation_id"),
		native_compaction_json: text("native_compaction_json"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		index("thread_run_continuation_state_thread_index").on(table.thread_id),
		check(
			"thread_run_continuation_state_sequence_check",
			sql`${table.last_observation_sequence} >= 0`,
		),
		check(
			"thread_run_continuation_state_compaction_shape_check",
			sql`(${table.native_compaction_boundary_journal_sequence} IS NULL AND ${table.native_compaction_observation_id} IS NULL AND ${table.native_compaction_json} IS NULL) OR (${table.native_compaction_boundary_journal_sequence} IS NOT NULL AND ${table.native_compaction_boundary_journal_sequence} >= 0 AND ${table.native_compaction_observation_id} IS NOT NULL AND (${table.native_compaction_json} IS NULL OR length(${table.native_compaction_json}) > 0))`,
		),
	],
);

/** Stores immutable private content transferred into one fresh target native session. */
export const ThreadPortableHandoffs = sqliteTable(
	"thread_portable_handoffs",
	{
		handoff_id: text("handoff_id").primaryKey(),
		target_run_id: text("target_run_id").notNull(),
		thread_id: text("thread_id").notNull(),
		source_run_id: text("source_run_id").notNull(),
		source_engine_id: text("source_engine_id").notNull(),
		source_model_id: text("source_model_id"),
		through_journal_sequence: integer("through_journal_sequence").notNull(),
		through_observation_sequence: integer("through_observation_sequence").notNull(),
		through_native_turn_id: text("through_native_turn_id"),
		method: text("method").notNull(),
		omitted_entries: integer("omitted_entries").notNull(),
		summary: text("summary").notNull(),
		tail_json: text("tail_json").notNull(),
		provider_lineage_json: text("provider_lineage_json").notNull(),
		content_sha256: text("content_sha256").notNull(),
		created_at: text("created_at").notNull(),
	},
	(table) => [
		uniqueIndex("thread_portable_handoffs_target_run_unique").on(table.target_run_id),
		index("thread_portable_handoffs_thread_index").on(table.thread_id, table.created_at),
		check(
			"thread_portable_handoffs_cut_check",
			sql`${table.through_journal_sequence} >= 0 AND ${table.through_observation_sequence} >= 0`,
		),
		check("thread_portable_handoffs_omission_check", sql`${table.omitted_entries} >= 0`),
		check(
			"thread_portable_handoffs_method_check",
			sql`${table.method} IN ('claude_post_compact', 'codex_fork_summary', 'canonical_transcript_summary')`,
		),
		check(
			"thread_portable_handoffs_hash_check",
			sql`length(${table.content_sha256}) = 64 AND ${table.content_sha256} NOT GLOB '*[^0-9a-f]*'`,
		),
		check(
			"thread_portable_handoffs_lineage_check",
			sql`length(${table.provider_lineage_json}) > 0`,
		),
	],
);

/** Tracks one crash-visible, idempotent continuation decision for a target run. */
export const ThreadContinuationLaunches = sqliteTable(
	"thread_continuation_launches",
	{
		target_run_id: text("target_run_id").primaryKey(),
		request_id: text("request_id").notNull(),
		thread_id: text("thread_id").notNull(),
		source_run_id: text("source_run_id"),
		source_kind: text("source_kind").notNull(),
		handoff_id: text("handoff_id"),
		target_engine_id: text("target_engine_id").notNull(),
		target_model_id: text("target_model_id"),
		state: text("state").notNull(),
		failure_code: text("failure_code"),
		created_at: text("created_at").notNull(),
		updated_at: text("updated_at").notNull(),
	},
	(table) => [
		uniqueIndex("thread_continuation_launches_request_unique").on(table.request_id),
		index("thread_continuation_launches_thread_index").on(table.thread_id, table.created_at),
		check(
			"thread_continuation_launches_source_kind_check",
			sql`${table.source_kind} IN ('fresh', 'native', 'portable')`,
		),
		check(
			"thread_continuation_launches_state_check",
			sql`${table.state} IN ('prepared', 'opening', 'bound', 'failed')`,
		),
		check(
			"thread_continuation_launches_source_shape_check",
			sql`(${table.source_kind} = 'fresh' AND ${table.source_run_id} IS NULL AND ${table.handoff_id} IS NULL) OR (${table.source_kind} = 'native' AND ${table.source_run_id} IS NOT NULL AND ${table.handoff_id} IS NULL) OR (${table.source_kind} = 'portable' AND ${table.source_run_id} IS NOT NULL AND ${table.handoff_id} IS NOT NULL)`,
		),
		check(
			"thread_continuation_launches_failure_check",
			sql`(${table.state} = 'failed') = (${table.failure_code} IS NOT NULL)`,
		),
	],
);
