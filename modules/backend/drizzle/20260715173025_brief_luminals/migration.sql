CREATE TABLE `preview_browser_launches` (
	`message_id` text PRIMARY KEY,
	`command_json` text NOT NULL,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`target_id` text NOT NULL,
	`target_generation_id` text NOT NULL,
	`url` text NOT NULL,
	`initiator_kind` text NOT NULL,
	`initiator_agent_id` text,
	`claim_token` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`lease_expires_at_ms` integer NOT NULL,
	`state` text NOT NULL,
	`reason` text,
	`requested_at_ms` integer NOT NULL,
	`updated_at_ms` integer NOT NULL,
	CONSTRAINT "preview_browser_launches_initiator_check" CHECK(COALESCE(("initiator_kind" = 'user' AND "initiator_agent_id" IS NULL) OR ("initiator_kind" = 'agent' AND "initiator_agent_id" IS NOT NULL), 0)),
	CONSTRAINT "preview_browser_launches_state_check" CHECK("state" IN ('accepted', 'dispatching', 'dispatched', 'outcome_unknown', 'rejected')),
	CONSTRAINT "preview_browser_launches_reason_check" CHECK(COALESCE(("state" IN ('accepted', 'dispatching', 'dispatched') AND "reason" IS NULL) OR ("state" = 'outcome_unknown' AND "reason" IN ('interrupted', 'launcher_failed')) OR ("state" = 'rejected' AND "reason" IN ('launcher_rejected', 'launcher_unavailable', 'target_changed')), 0)),
	CONSTRAINT "preview_browser_launches_timestamp_check" CHECK("requested_at_ms" >= 0 AND "updated_at_ms" >= "requested_at_ms")
);
--> statement-breakpoint
CREATE TABLE `preview_inspection_sessions` (
	`inspection_id` text PRIMARY KEY,
	`attach_message_id` text NOT NULL,
	`attach_command_json` text NOT NULL,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`target_id` text NOT NULL,
	`target_generation_id` text NOT NULL,
	`url` text NOT NULL,
	`connector_id` text NOT NULL,
	`initiator_kind` text NOT NULL,
	`initiator_agent_id` text,
	`claim_token` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`lease_expires_at_ms` integer NOT NULL,
	`state` text NOT NULL,
	`reason` text,
	`requested_at_ms` integer NOT NULL,
	`updated_at_ms` integer NOT NULL,
	CONSTRAINT "preview_inspection_sessions_initiator_check" CHECK(COALESCE(("initiator_kind" = 'user' AND "initiator_agent_id" IS NULL) OR ("initiator_kind" = 'agent' AND "initiator_agent_id" IS NOT NULL), 0)),
	CONSTRAINT "preview_inspection_sessions_state_check" CHECK("state" IN ('attached', 'attaching', 'disconnected', 'failed')),
	CONSTRAINT "preview_inspection_sessions_reason_check" CHECK(COALESCE(("state" IN ('attached', 'attaching') AND "reason" IS NULL) OR ("state" = 'failed' AND "reason" IN ('connector_rejected', 'connector_unavailable', 'target_changed')) OR ("state" = 'disconnected' AND "reason" IN ('connection_lost', 'detached', 'interrupted', 'target_changed', 'thread_erased')), 0)),
	CONSTRAINT "preview_inspection_sessions_timestamp_check" CHECK("requested_at_ms" >= 0 AND "updated_at_ms" >= "requested_at_ms")
);
--> statement-breakpoint
CREATE TABLE `preview_target_removal_claims` (
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`target_id` text NOT NULL,
	`claim_token` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`lease_expires_at_ms` integer NOT NULL,
	`created_at_ms` integer NOT NULL,
	`updated_at_ms` integer NOT NULL,
	CONSTRAINT `preview_target_removal_claims_pk` PRIMARY KEY(`project_id`, `workspace_id`, `target_id`),
	CONSTRAINT "preview_target_removal_claims_timestamp_check" CHECK("created_at_ms" >= 0 AND "updated_at_ms" >= "created_at_ms" AND "lease_expires_at_ms" >= "updated_at_ms")
);
--> statement-breakpoint
CREATE TABLE `preview_target_removal_fences` (
	`message_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`target_id` text NOT NULL,
	`target_generation_id` text NOT NULL,
	`committed_at_ms` integer NOT NULL,
	CONSTRAINT "preview_target_removal_fences_timestamp_check" CHECK("committed_at_ms" >= 0)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `preview_browser_launches_claim_token_unique` ON `preview_browser_launches` (`claim_token`);--> statement-breakpoint
CREATE INDEX `preview_browser_launches_lease_index` ON `preview_browser_launches` (`lease_expires_at_ms`);--> statement-breakpoint
CREATE INDEX `preview_browser_launches_scope_index` ON `preview_browser_launches` (`project_id`,`workspace_id`,`updated_at_ms`);--> statement-breakpoint
CREATE INDEX `preview_browser_launches_thread_index` ON `preview_browser_launches` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_browser_launches_target_index` ON `preview_browser_launches` (`project_id`,`workspace_id`,`target_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `preview_inspection_sessions_attach_message_unique` ON `preview_inspection_sessions` (`attach_message_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `preview_inspection_sessions_claim_token_unique` ON `preview_inspection_sessions` (`claim_token`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_lease_index` ON `preview_inspection_sessions` (`lease_expires_at_ms`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_scope_index` ON `preview_inspection_sessions` (`project_id`,`workspace_id`,`updated_at_ms`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_thread_index` ON `preview_inspection_sessions` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_target_index` ON `preview_inspection_sessions` (`project_id`,`workspace_id`,`target_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `preview_target_removal_claims_token_unique` ON `preview_target_removal_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `preview_target_removal_claims_lease_index` ON `preview_target_removal_claims` (`lease_expires_at_ms`);--> statement-breakpoint
CREATE UNIQUE INDEX `preview_target_removal_fences_scope_unique` ON `preview_target_removal_fences` (`project_id`,`workspace_id`,`target_id`);--> statement-breakpoint
CREATE INDEX `preview_target_removal_fences_thread_index` ON `preview_target_removal_fences` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_target_removal_fences_generation_scope_index` ON `preview_target_removal_fences` (`project_id`,`workspace_id`,`target_id`,`target_generation_id`);