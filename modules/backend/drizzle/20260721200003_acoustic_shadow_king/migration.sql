ALTER TABLE `orchestration_coordinators` ADD `policy_model` text;--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_reasoning_effort` text DEFAULT 'medium' NOT NULL;--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_permission_mode` text DEFAULT 'on_request' NOT NULL;--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_sandbox_mode` text DEFAULT 'workspace_write' NOT NULL;--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_web_search_enabled` integer DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_strict_clarification` integer DEFAULT false NOT NULL;
