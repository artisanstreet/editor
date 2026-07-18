CREATE TABLE `artisan_tool_approvals` (
	`approval_id` text PRIMARY KEY,
	`invocation_id` text NOT NULL,
	`request_json` text NOT NULL,
	`state` text NOT NULL,
	`resolution_json` text,
	`resolution_id` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "artisan_tool_approvals_state_check" CHECK("state" IN ('pending', 'resolved')),
	CONSTRAINT "artisan_tool_approvals_resolution_check" CHECK(("state" = 'resolved') = ("resolution_json" IS NOT NULL AND "resolution_id" IS NOT NULL))
);
--> statement-breakpoint
CREATE TABLE `artisan_tool_invocations` (
	`invocation_id` text PRIMARY KEY,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`tool_id` text NOT NULL,
	`input_summary` text NOT NULL,
	`permission_json` text NOT NULL,
	`raw_origin_json` text,
	`approval_id` text,
	`workspace_evidence_json` text,
	`lifecycle` text NOT NULL,
	`outcome_json` text,
	`claim_owner_id` text,
	`claim_lease_expires_at` text,
	`requested_at` text NOT NULL,
	`completed_at` text,
	`updated_at` text NOT NULL,
	`journal_sequence` integer,
	CONSTRAINT "artisan_tool_invocations_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "artisan_tool_invocations_lifecycle_check" CHECK("lifecycle" IN ('requested', 'awaiting_approval', 'running', 'succeeded', 'denied', 'failed', 'cancelled', 'unsupported')),
	CONSTRAINT "artisan_tool_invocations_terminal_check" CHECK(("lifecycle" IN ('succeeded', 'denied', 'failed', 'cancelled', 'unsupported')) = ("outcome_json" IS NOT NULL))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `artisan_tool_approvals_invocation_unique` ON `artisan_tool_approvals` (`invocation_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `artisan_tool_approvals_resolution_unique` ON `artisan_tool_approvals` (`resolution_id`);--> statement-breakpoint
CREATE INDEX `artisan_tool_approvals_state_index` ON `artisan_tool_approvals` (`state`);--> statement-breakpoint
CREATE UNIQUE INDEX `artisan_tool_invocations_approval_unique` ON `artisan_tool_invocations` (`approval_id`);--> statement-breakpoint
CREATE INDEX `artisan_tool_invocations_thread_index` ON `artisan_tool_invocations` (`thread_id`,`requested_at`);--> statement-breakpoint
CREATE INDEX `artisan_tool_invocations_thread_lifecycle_index` ON `artisan_tool_invocations` (`thread_id`,`lifecycle`);