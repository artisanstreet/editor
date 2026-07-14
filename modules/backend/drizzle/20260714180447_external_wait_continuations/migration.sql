ALTER TABLE `agent_runs` ADD `continuation_index` integer DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE `agent_runs` ADD `continuation_text` text;--> statement-breakpoint
ALTER TABLE `agent_runs` ADD `open_mode` text DEFAULT 'start' NOT NULL;--> statement-breakpoint
ALTER TABLE `orchestration_runs` ADD `open_mode` text DEFAULT 'start' NOT NULL;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_agent_runs` (
	`run_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`assignment_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`attempt` integer NOT NULL,
	`continuation_index` integer DEFAULT 0 NOT NULL,
	`continuation_text` text,
	`engine_id` text NOT NULL,
	`open_mode` text DEFAULT 'start' NOT NULL,
	`profile` text NOT NULL,
	`state` text NOT NULL,
	`dispatch_status` text NOT NULL,
	`owner_instance_id` text,
	`native_thread_id` text,
	`native_resume_json` text,
	`native_identity_json` text,
	`raw_origin_json` text,
	`last_observation_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`completed_at` text,
	CONSTRAINT "agent_runs_continuation_index_check" CHECK("continuation_index" >= 0),
	CONSTRAINT "agent_runs_open_mode_check" CHECK("open_mode" IN ('start', 'resume'))
);
--> statement-breakpoint
INSERT INTO `__new_agent_runs`(`run_id`, `group_id`, `assignment_id`, `agent_id`, `attempt`, `engine_id`, `profile`, `state`, `dispatch_status`, `owner_instance_id`, `native_thread_id`, `native_resume_json`, `native_identity_json`, `raw_origin_json`, `last_observation_sequence`, `created_at`, `updated_at`, `completed_at`) SELECT `run_id`, `group_id`, `assignment_id`, `agent_id`, `attempt`, `engine_id`, `profile`, `state`, `dispatch_status`, `owner_instance_id`, `native_thread_id`, `native_resume_json`, `native_identity_json`, `raw_origin_json`, `last_observation_sequence`, `created_at`, `updated_at`, `completed_at` FROM `agent_runs`;--> statement-breakpoint
DROP TABLE `agent_runs`;--> statement-breakpoint
ALTER TABLE `__new_agent_runs` RENAME TO `agent_runs`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_orchestration_runs` (
	`run_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`working_directory` text NOT NULL,
	`status` text NOT NULL,
	`open_mode` text DEFAULT 'start' NOT NULL,
	`native_thread_id` text,
	`native_resume_json` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "orchestration_runs_open_mode_check" CHECK("open_mode" IN ('start', 'resume'))
);
--> statement-breakpoint
INSERT INTO `__new_orchestration_runs`(`run_id`, `thread_id`, `agent_id`, `engine_id`, `working_directory`, `status`, `native_thread_id`, `native_resume_json`, `created_at`, `updated_at`) SELECT `run_id`, `thread_id`, `agent_id`, `engine_id`, `working_directory`, `status`, `native_thread_id`, `native_resume_json`, `created_at`, `updated_at` FROM `orchestration_runs`;--> statement-breakpoint
DROP TABLE `orchestration_runs`;--> statement-breakpoint
ALTER TABLE `__new_orchestration_runs` RENAME TO `orchestration_runs`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
DROP INDEX IF EXISTS `agent_runs_assignment_attempt_unique`;--> statement-breakpoint
CREATE UNIQUE INDEX `agent_runs_assignment_attempt_continuation_unique` ON `agent_runs` (`assignment_id`,`attempt`,`continuation_index`);--> statement-breakpoint
CREATE INDEX `agent_runs_group_id_index` ON `agent_runs` (`group_id`);--> statement-breakpoint
CREATE INDEX `agent_runs_dispatch_status_index` ON `agent_runs` (`dispatch_status`);--> statement-breakpoint
CREATE INDEX `agent_runs_assignment_id_index` ON `agent_runs` (`assignment_id`);--> statement-breakpoint
CREATE INDEX `orchestration_runs_thread_id_index` ON `orchestration_runs` (`thread_id`);--> statement-breakpoint
CREATE INDEX `orchestration_runs_status_index` ON `orchestration_runs` (`status`);