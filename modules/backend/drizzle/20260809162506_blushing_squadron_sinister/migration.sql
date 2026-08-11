CREATE TABLE `native_subagent_observation_inbox` (
	`observation_id` text PRIMARY KEY,
	`root_run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`agent_native_thread_id` text NOT NULL,
	`parent_native_thread_id` text NOT NULL,
	`state` text NOT NULL,
	`sequence` integer NOT NULL,
	`activity` text,
	`agent_path` text,
	`turn_id` text,
	`native_id` text,
	`created_at` text NOT NULL,
	`processed_at` text
);
--> statement-breakpoint
CREATE INDEX `native_subagent_observation_inbox_pending_index` ON `native_subagent_observation_inbox` (`processed_at`,`root_run_id`,`sequence`);