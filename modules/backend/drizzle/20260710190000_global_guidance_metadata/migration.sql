CREATE TABLE `global_guidance_canonical` (
	`canonical_id` integer PRIMARY KEY,
	`content_hash` text,
	`byte_count` integer,
	`status` text NOT NULL,
	`selected_provider` text,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
INSERT INTO `global_guidance_canonical` (
	`canonical_id`, `content_hash`, `byte_count`, `status`, `selected_provider`, `updated_at`
) VALUES (1, NULL, NULL, 'initialization_required', NULL, '1970-01-01T00:00:00.000Z');
--> statement-breakpoint
CREATE TABLE `global_guidance_provider_sync` (
	`provider` text PRIMARY KEY,
	`status` text NOT NULL,
	`path` text,
	`modified_at` text,
	`observed_hash` text,
	`observed_byte_count` integer,
	`applied_hash` text,
	`applied_byte_count` integer,
	`ignored_drift_hash` text,
	`backup_path` text,
	`last_error_code` text,
	`updated_at` text NOT NULL
);
