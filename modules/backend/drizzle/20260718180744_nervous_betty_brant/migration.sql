CREATE TABLE `marketplace_routine_mirrors` (
	`routine_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`status` text NOT NULL,
	`observed_revision` text,
	`last_error_code` text,
	`updated_at` text NOT NULL,
	CONSTRAINT `marketplace_routine_mirrors_pk` PRIMARY KEY(`routine_id`, `engine_id`)
);
--> statement-breakpoint
CREATE TABLE `marketplace_routine_operations` (
	`operation_id` text PRIMARY KEY,
	`kind` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`approval_id` text,
	`approval_fingerprint` text,
	`approval_decision` text,
	`routine_id` text,
	`preview_json` text,
	`detail_json` text,
	`rollback_json` text,
	`dispatch_owner_id` text,
	`dispatch_lease_expires_at` text,
	`state` text NOT NULL,
	`failure_code` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `marketplace_routines` (
	`id` text PRIMARY KEY,
	`display_name` text NOT NULL,
	`description` text NOT NULL,
	`instructions` text NOT NULL,
	`source_json` text NOT NULL,
	`version` text NOT NULL,
	`author` text,
	`scope_json` text NOT NULL,
	`permissions_json` text NOT NULL,
	`compatibility_json` text NOT NULL,
	`commands_json` text NOT NULL,
	`files_json` text NOT NULL,
	`trust` text NOT NULL,
	`enabled` integer NOT NULL,
	`status` text NOT NULL,
	`artifact_refs_json` text DEFAULT '[]' NOT NULL,
	`removed_at` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `marketplace_routine_operations_approval_unique` ON `marketplace_routine_operations` (`approval_id`);--> statement-breakpoint
CREATE INDEX `marketplace_routine_operations_routine_index` ON `marketplace_routine_operations` (`routine_id`);--> statement-breakpoint
CREATE INDEX `marketplace_routines_scope_index` ON `marketplace_routines` (`scope_json`);--> statement-breakpoint
CREATE INDEX `marketplace_routines_enabled_index` ON `marketplace_routines` (`enabled`);
