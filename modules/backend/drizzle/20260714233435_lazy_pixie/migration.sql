CREATE TABLE `preview_target_probe_claims` (
	`message_id` text PRIMARY KEY,
	`command_json` text NOT NULL,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`target_id` text NOT NULL,
	`target_generation_id` text NOT NULL,
	`claim_token` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`lease_expires_at` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `preview_targets` (
	`target_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`generation_id` text NOT NULL,
	`url` text NOT NULL,
	`source_kind` text,
	`source_id` text,
	`state` text NOT NULL,
	`health_status` text,
	`health_checked_at_ms` integer,
	`health_latency_ms` integer,
	`health_message` text,
	`health_status_code` integer,
	`created_at_ms` integer NOT NULL,
	`updated_at_ms` integer NOT NULL,
	CONSTRAINT `preview_targets_pk` PRIMARY KEY(`project_id`, `workspace_id`, `target_id`),
	CONSTRAINT "preview_targets_source_check" CHECK(COALESCE(("source_kind" IS NULL AND "source_id" IS NULL) OR ("source_kind" IS NOT NULL AND "source_kind" IN ('process', 'terminal') AND "source_id" IS NOT NULL), 0)),
	CONSTRAINT "preview_targets_state_check" CHECK("state" IN ('healthy', 'registered', 'stopped', 'unhealthy')),
	CONSTRAINT "preview_targets_health_check" CHECK(COALESCE(("health_status" IS NULL AND "health_checked_at_ms" IS NULL AND "health_latency_ms" IS NULL AND "health_message" IS NULL AND "health_status_code" IS NULL) OR ("health_status" IS NOT NULL AND "health_status" IN ('healthy', 'unhealthy') AND "health_checked_at_ms" IS NOT NULL AND "health_latency_ms" IS NOT NULL AND "health_latency_ms" >= 0), 0))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `preview_target_probe_claims_token_unique` ON `preview_target_probe_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `preview_target_probe_claims_thread_index` ON `preview_target_probe_claims` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_target_probe_claims_lease_index` ON `preview_target_probe_claims` (`lease_expires_at`);