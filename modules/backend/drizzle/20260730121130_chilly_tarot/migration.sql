CREATE TABLE `thread_continuation_launches` (
	`target_run_id` text PRIMARY KEY,
	`request_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`source_run_id` text,
	`source_kind` text NOT NULL,
	`handoff_id` text,
	`target_engine_id` text NOT NULL,
	`target_model_id` text,
	`state` text NOT NULL,
	`failure_code` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "thread_continuation_launches_source_kind_check" CHECK("source_kind" IN ('fresh', 'native', 'portable')),
	CONSTRAINT "thread_continuation_launches_state_check" CHECK("state" IN ('prepared', 'opening', 'bound', 'failed')),
	CONSTRAINT "thread_continuation_launches_source_shape_check" CHECK(("source_kind" = 'fresh' AND "source_run_id" IS NULL AND "handoff_id" IS NULL) OR ("source_kind" = 'native' AND "source_run_id" IS NOT NULL AND "handoff_id" IS NULL) OR ("source_kind" = 'portable' AND "source_run_id" IS NOT NULL AND "handoff_id" IS NOT NULL)),
	CONSTRAINT "thread_continuation_launches_failure_check" CHECK(("state" = 'failed') = ("failure_code" IS NOT NULL))
);
--> statement-breakpoint
CREATE TABLE `thread_portable_handoffs` (
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
	CONSTRAINT "thread_portable_handoffs_method_check" CHECK("method" IN ('claude_post_compact', 'codex_fork_summary', 'canonical_transcript_summary')),
	CONSTRAINT "thread_portable_handoffs_hash_check" CHECK(length("content_sha256") = 64 AND "content_sha256" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "thread_portable_handoffs_lineage_check" CHECK(length("provider_lineage_json") > 0)
);
--> statement-breakpoint
CREATE TABLE `thread_run_continuation_state` (
	`run_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`model_id` text,
	`last_native_turn_id` text,
	`last_observation_sequence` integer DEFAULT 0 NOT NULL,
	`native_compaction_boundary_journal_sequence` integer,
	`native_compaction_observation_id` text,
	`native_compaction_json` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "thread_run_continuation_state_sequence_check" CHECK("last_observation_sequence" >= 0),
	CONSTRAINT "thread_run_continuation_state_compaction_shape_check" CHECK(("native_compaction_boundary_journal_sequence" IS NULL AND "native_compaction_observation_id" IS NULL AND "native_compaction_json" IS NULL) OR ("native_compaction_boundary_journal_sequence" IS NOT NULL AND "native_compaction_boundary_journal_sequence" >= 0 AND "native_compaction_observation_id" IS NOT NULL AND ("native_compaction_json" IS NULL OR length("native_compaction_json") > 0)))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `thread_continuation_launches_request_unique` ON `thread_continuation_launches` (`request_id`);--> statement-breakpoint
CREATE INDEX `thread_continuation_launches_thread_index` ON `thread_continuation_launches` (`thread_id`,`created_at`);--> statement-breakpoint
CREATE UNIQUE INDEX `thread_portable_handoffs_target_run_unique` ON `thread_portable_handoffs` (`target_run_id`);--> statement-breakpoint
CREATE INDEX `thread_portable_handoffs_thread_index` ON `thread_portable_handoffs` (`thread_id`,`created_at`);--> statement-breakpoint
CREATE INDEX `thread_run_continuation_state_thread_index` ON `thread_run_continuation_state` (`thread_id`);