CREATE TABLE `marketplace_capabilities` (
	`id` text PRIMARY KEY,
	`display_name` text NOT NULL,
	`source_json` text NOT NULL,
	`transport_json` text NOT NULL,
	`auth_json` text NOT NULL,
	`scope_json` text NOT NULL,
	`permissions_json` text NOT NULL,
	`compatibility_json` text NOT NULL,
	`tools_json` text NOT NULL,
	`resources_json` text NOT NULL,
	`instructions` text,
	`policy_json` text NOT NULL,
	`trust` text NOT NULL,
	`enabled` integer NOT NULL,
	`status` text NOT NULL,
	`lifecycle` text NOT NULL,
	`health_json` text NOT NULL,
	`raw_provider_metadata_json` text,
	`removed_at` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `marketplace_capability_artifacts` (
	`artifact_id` text PRIMARY KEY,
	`capability_id` text NOT NULL,
	`operation_id` text NOT NULL,
	`tool_name` text NOT NULL,
	`result_json` text NOT NULL,
	`created_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `marketplace_capability_mirrors` (
	`capability_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`status` text NOT NULL,
	`observed_revision` text,
	`last_error_code` text,
	`updated_at` text NOT NULL,
	CONSTRAINT `marketplace_capability_mirrors_pk` PRIMARY KEY(`capability_id`, `engine_id`)
);
--> statement-breakpoint
CREATE TABLE `marketplace_capability_operations` (
	`operation_id` text PRIMARY KEY,
	`capability_id` text NOT NULL,
	`kind` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`approval_id` text,
	`approval_fingerprint` text,
	`approval_decision` text,
	`preview_json` text,
	`detail_json` text,
	`state` text NOT NULL,
	`failure_code` text,
	`artifact_id` text,
	`tool_name` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `marketplace_capabilities_scope_index` ON `marketplace_capabilities` (`scope_json`);--> statement-breakpoint
CREATE INDEX `marketplace_capabilities_enabled_index` ON `marketplace_capabilities` (`enabled`);--> statement-breakpoint
CREATE INDEX `marketplace_capabilities_lifecycle_index` ON `marketplace_capabilities` (`lifecycle`);--> statement-breakpoint
CREATE UNIQUE INDEX `marketplace_capability_artifacts_operation_unique` ON `marketplace_capability_artifacts` (`operation_id`);--> statement-breakpoint
CREATE INDEX `marketplace_capability_artifacts_capability_index` ON `marketplace_capability_artifacts` (`capability_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `marketplace_capability_operations_approval_unique` ON `marketplace_capability_operations` (`approval_id`);--> statement-breakpoint
CREATE INDEX `marketplace_capability_operations_capability_index` ON `marketplace_capability_operations` (`capability_id`);