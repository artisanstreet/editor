CREATE TABLE `native_subagent_bindings` (
	`binding_id` text PRIMARY KEY,
	`engine_id` text NOT NULL,
	`root_run_id` text NOT NULL,
	`group_id` text NOT NULL,
	`parent_native_thread_id` text NOT NULL,
	`agent_native_thread_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`assignment_id` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_path` text,
	`activity` text,
	`turn_id` text,
	`state` text NOT NULL,
	`raw_origin_json` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
ALTER TABLE `agent_runs` ADD `execution_origin` text DEFAULT 'artisan_dispatched' NOT NULL;--> statement-breakpoint
CREATE UNIQUE INDEX `native_subagent_bindings_identity_unique` ON `native_subagent_bindings` (`engine_id`,`root_run_id`,`agent_native_thread_id`);--> statement-breakpoint
CREATE INDEX `native_subagent_bindings_group_id_index` ON `native_subagent_bindings` (`group_id`);--> statement-breakpoint
CREATE INDEX `native_subagent_bindings_root_run_id_index` ON `native_subagent_bindings` (`root_run_id`);
