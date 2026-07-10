CREATE TABLE `orchestration_coordinators` (
	`thread_id` text PRIMARY KEY,
	`agent_id` text NOT NULL,
	`role` text NOT NULL,
	`display_name` text NOT NULL,
	`engine_id` text NOT NULL,
	`active_run_id` text,
	`native_thread_id` text,
	`native_resume_json` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
ALTER TABLE `journal_commands` ADD `assigned_run_id` text;
--> statement-breakpoint
CREATE TABLE `orchestration_interactions` (
	`interaction_id` text NOT NULL,
	`run_id` text NOT NULL,
	`kind` text NOT NULL,
	`description` text NOT NULL,
	`state` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	PRIMARY KEY(`run_id`, `kind`, `interaction_id`)
);
--> statement-breakpoint
CREATE TABLE `orchestration_raw_observations` (
	`observation_id` text PRIMARY KEY,
	`run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`native_id` text,
	`native_method` text,
	`transport` text NOT NULL,
	`protocol_version` text,
	`frame_json` text NOT NULL,
	`raw_frame_base64` text
);
--> statement-breakpoint
CREATE TABLE `orchestration_messages` (
	`message_id` text PRIMARY KEY,
	`command_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text NOT NULL,
	`text` text NOT NULL,
	`delivery` text NOT NULL,
	`created_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `orchestration_outbox` (
	`command_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`kind` text NOT NULL,
	`payload_json` text NOT NULL,
	`status` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `orchestration_runs` (
	`run_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`working_directory` text NOT NULL,
	`status` text NOT NULL,
	`native_thread_id` text,
	`native_resume_json` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_outbox_status_index` ON `orchestration_outbox` (`status`);--> statement-breakpoint
CREATE INDEX `orchestration_outbox_run_id_index` ON `orchestration_outbox` (`run_id`);--> statement-breakpoint
CREATE INDEX `orchestration_runs_thread_id_index` ON `orchestration_runs` (`thread_id`);--> statement-breakpoint
CREATE INDEX `orchestration_runs_status_index` ON `orchestration_runs` (`status`);
--> statement-breakpoint
CREATE INDEX `orchestration_raw_observations_run_sequence_index` ON `orchestration_raw_observations` (`run_id`,`sequence`);
