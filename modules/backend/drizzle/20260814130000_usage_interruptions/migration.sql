ALTER TABLE `session_defaults` ADD `auto_continue_usage_limits` integer DEFAULT true NOT NULL;
--> statement-breakpoint
CREATE TABLE `usage_interruptions` (
	`interruption_id` text PRIMARY KEY NOT NULL,
	`thread_id` text NOT NULL,
	`source_run_id` text NOT NULL,
	`source_agent_id` text NOT NULL,
	`source_engine_id` text NOT NULL,
	`source_model_id` text,
	`provider_code` text,
	`limit_scope` text NOT NULL,
	`limit_id` text,
	`limit_label` text,
	`affected_model_id` text,
	`resets_at` text,
	`auto_continue` integer NOT NULL,
	`resume_not_before` text,
	`state` text NOT NULL,
	`revision` integer DEFAULT 0 NOT NULL,
	`alternatives_json` text DEFAULT '[]' NOT NULL,
	`continuation_command_id` text,
	`target_run_id` text,
	`target_engine_id` text,
	`target_model_id` text,
	`created_at` text NOT NULL,
	`evidence_refreshed_at` text,
	`updated_at` text NOT NULL,
	`continued_at` text,
	`cancelled_at` text,
	`failed_at` text,
	CONSTRAINT `usage_interruptions_limit_scope_check` CHECK (`limit_scope` IN ('shared', 'model', 'unknown')),
	CONSTRAINT `usage_interruptions_state_check` CHECK (`state` IN ('scheduled', 'awaiting_decision', 'launching', 'continued', 'cancelled', 'failed'))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `usage_interruptions_source_run_unique` ON `usage_interruptions` (`source_run_id`);
--> statement-breakpoint
CREATE INDEX `usage_interruptions_due_index` ON `usage_interruptions` (`state`,`resume_not_before`);
--> statement-breakpoint
CREATE INDEX `usage_interruptions_thread_index` ON `usage_interruptions` (`thread_id`);
