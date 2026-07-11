CREATE TABLE `workspace_change_operations` (
	`message_id` text PRIMARY KEY,
	`action` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`change_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`raw_origin_json` text,
	`workspace_id` text,
	`path` text,
	`expected_identity_json` text,
	`result_identity_json` text,
	`lifecycle` text NOT NULL,
	`evidence_recorded` integer NOT NULL DEFAULT false,
	`journal_sequence` integer,
	`sent_at` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_change_operations_change_action_unique`
ON `workspace_change_operations` (`change_id`, `action`);
--> statement-breakpoint
CREATE INDEX `workspace_change_operations_change_id_index`
ON `workspace_change_operations` (`change_id`);
--> statement-breakpoint
CREATE TABLE `workspace_changes` (
	`change_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`before_identity_json` text NOT NULL,
	`after_identity_json` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`raw_origin_json` text,
	`review_state` text NOT NULL,
	`rollback_state` text NOT NULL,
	`reviewed_at` text,
	`rolled_back_at` text,
	`version` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_changes_source_command_unique`
ON `workspace_changes` (`source_command_id`);
--> statement-breakpoint
CREATE INDEX `workspace_changes_thread_id_index` ON `workspace_changes` (`thread_id`);
--> statement-breakpoint
CREATE INDEX `workspace_changes_thread_workspace_index`
ON `workspace_changes` (`thread_id`, `workspace_id`);
