CREATE TABLE `native_subagent_transcript_inbox` (
	`observation_id` text PRIMARY KEY,
	`root_run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`agent_native_thread_id` text NOT NULL,
	`parent_native_thread_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`content_json` text NOT NULL,
	`created_at` text NOT NULL,
	`processed_at` text
);
--> statement-breakpoint
CREATE INDEX `native_subagent_transcript_inbox_pending_index` ON `native_subagent_transcript_inbox` (`processed_at`,`root_run_id`,`agent_native_thread_id`,`sequence`);