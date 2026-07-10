CREATE TABLE `terminal_sessions` (
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
	`owner_instance_id` text NOT NULL,
	`state` text NOT NULL,
	`exit_code` integer,
	`exit_signal` integer,
	`exit_reason` text,
	`failure` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`closed_at` text
);
--> statement-breakpoint
CREATE INDEX `terminal_sessions_thread_workspace_index` ON `terminal_sessions` (`thread_id`,`workspace_id`);
--> statement-breakpoint
CREATE INDEX `terminal_sessions_state_index` ON `terminal_sessions` (`state`);
--> statement-breakpoint
CREATE TABLE `terminal_commands` (
	`message_id` text PRIMARY KEY,
	`terminal_id` text NOT NULL,
	`generation` integer NOT NULL,
	`claimed_session_json` text NOT NULL,
	`payload_json` text NOT NULL,
	`status` text NOT NULL,
	`journal_sequence` integer,
	`failure` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
