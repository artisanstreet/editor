ALTER TABLE `terminal_sessions` ADD `owner_kind` text DEFAULT 'user' NOT NULL;--> statement-breakpoint
ALTER TABLE `terminal_sessions` ADD `owner_agent_id` text;--> statement-breakpoint
ALTER TABLE `terminal_sessions` ADD `owner_run_id` text;--> statement-breakpoint
ALTER TABLE `terminal_sessions` ADD `pinned` integer DEFAULT false NOT NULL;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_terminal_sessions` (
	`terminal_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`working_directory` text NOT NULL,
	`executable` text NOT NULL,
	`args_json` text NOT NULL,
	`env_json` text,
	`cols` integer NOT NULL,
	`generation` integer NOT NULL,
	`rows` integer NOT NULL,
	`pid` integer,
	`owner_kind` text DEFAULT 'user' NOT NULL,
	`owner_agent_id` text,
	`owner_run_id` text,
	`pinned` integer DEFAULT false NOT NULL,
	`owner_instance_id` text NOT NULL,
	`state` text NOT NULL,
	`exit_code` integer,
	`exit_signal` integer,
	`exit_reason` text,
	`failure` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`closed_at` text,
	CONSTRAINT "terminal_sessions_owner_kind_check" CHECK("owner_kind" IN ('user', 'agent')),
	CONSTRAINT "terminal_sessions_owner_identity_check" CHECK(("owner_kind" = 'user' AND "owner_agent_id" IS NULL AND "owner_run_id" IS NULL) OR ("owner_kind" = 'agent' AND "owner_agent_id" IS NOT NULL AND "owner_run_id" IS NOT NULL))
);
--> statement-breakpoint
INSERT INTO `__new_terminal_sessions`(`terminal_id`, `thread_id`, `workspace_id`, `working_directory`, `executable`, `args_json`, `env_json`, `cols`, `generation`, `rows`, `pid`, `owner_instance_id`, `state`, `exit_code`, `exit_signal`, `exit_reason`, `failure`, `created_at`, `updated_at`, `closed_at`) SELECT `terminal_id`, `thread_id`, `workspace_id`, `working_directory`, `executable`, `args_json`, `env_json`, `cols`, `generation`, `rows`, `pid`, `owner_instance_id`, `state`, `exit_code`, `exit_signal`, `exit_reason`, `failure`, `created_at`, `updated_at`, `closed_at` FROM `terminal_sessions`;--> statement-breakpoint
DROP TABLE `terminal_sessions`;--> statement-breakpoint
ALTER TABLE `__new_terminal_sessions` RENAME TO `terminal_sessions`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `terminal_sessions_thread_workspace_index` ON `terminal_sessions` (`thread_id`,`workspace_id`);--> statement-breakpoint
CREATE INDEX `terminal_sessions_state_index` ON `terminal_sessions` (`state`);