ALTER TABLE `session_defaults` ADD `compaction_model_id` text;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_thread_run_continuation_state` (
	`run_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`model_id` text,
	`last_native_turn_id` text,
	`last_observation_sequence` integer DEFAULT 0 NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "thread_run_continuation_state_sequence_check" CHECK("last_observation_sequence" >= 0)
);
--> statement-breakpoint
INSERT INTO `__new_thread_run_continuation_state`(`run_id`, `thread_id`, `engine_id`, `model_id`, `last_native_turn_id`, `last_observation_sequence`, `created_at`, `updated_at`) SELECT `run_id`, `thread_id`, `engine_id`, `model_id`, `last_native_turn_id`, `last_observation_sequence`, `created_at`, `updated_at` FROM `thread_run_continuation_state`;--> statement-breakpoint
DROP TABLE `thread_run_continuation_state`;--> statement-breakpoint
ALTER TABLE `__new_thread_run_continuation_state` RENAME TO `thread_run_continuation_state`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_thread_portable_handoffs` (
	`handoff_id` text PRIMARY KEY,
	`target_run_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`source_run_id` text NOT NULL,
	`source_engine_id` text NOT NULL,
	`source_model_id` text,
	`through_journal_sequence` integer NOT NULL,
	`through_observation_sequence` integer NOT NULL,
	`through_native_turn_id` text,
	`method` text NOT NULL,
	`omitted_entries` integer NOT NULL,
	`summary` text NOT NULL,
	`tail_json` text NOT NULL,
	`provider_lineage_json` text NOT NULL,
	`content_sha256` text NOT NULL,
	`created_at` text NOT NULL,
	CONSTRAINT "thread_portable_handoffs_cut_check" CHECK("through_journal_sequence" >= 0 AND "through_observation_sequence" >= 0),
	CONSTRAINT "thread_portable_handoffs_omission_check" CHECK("omitted_entries" >= 0),
	CONSTRAINT "thread_portable_handoffs_method_check" CHECK("method" IN ('compaction_model_summary', 'canonical_transcript_summary', 'claude_post_compact', 'codex_fork_summary')),
	CONSTRAINT "thread_portable_handoffs_hash_check" CHECK(length("content_sha256") = 64 AND "content_sha256" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "thread_portable_handoffs_lineage_check" CHECK(length("provider_lineage_json") > 0)
);
--> statement-breakpoint
INSERT INTO `__new_thread_portable_handoffs`(`handoff_id`, `target_run_id`, `thread_id`, `source_run_id`, `source_engine_id`, `source_model_id`, `through_journal_sequence`, `through_observation_sequence`, `through_native_turn_id`, `method`, `omitted_entries`, `summary`, `tail_json`, `provider_lineage_json`, `content_sha256`, `created_at`) SELECT `handoff_id`, `target_run_id`, `thread_id`, `source_run_id`, `source_engine_id`, `source_model_id`, `through_journal_sequence`, `through_observation_sequence`, `through_native_turn_id`, `method`, `omitted_entries`, `summary`, `tail_json`, `provider_lineage_json`, `content_sha256`, `created_at` FROM `thread_portable_handoffs`;--> statement-breakpoint
DROP TABLE `thread_portable_handoffs`;--> statement-breakpoint
ALTER TABLE `__new_thread_portable_handoffs` RENAME TO `thread_portable_handoffs`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `thread_run_continuation_state_thread_index` ON `thread_run_continuation_state` (`thread_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `thread_portable_handoffs_target_run_unique` ON `thread_portable_handoffs` (`target_run_id`);--> statement-breakpoint
CREATE INDEX `thread_portable_handoffs_thread_index` ON `thread_portable_handoffs` (`thread_id`,`created_at`);