CREATE TABLE `git_mutation_operations` (
	`mutation_id` text PRIMARY KEY,
	`approval_id` text NOT NULL,
	`source_message_id` text NOT NULL,
	`decision_message_id` text,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`raw_origin_json` text,
	`workspace_id` text NOT NULL,
	`kind` text NOT NULL,
	`paths_json` text NOT NULL,
	`expected_snapshot_id` text NOT NULL,
	`expected_workspace_version` integer NOT NULL,
	`lifecycle` text NOT NULL,
	`failure_code` text,
	`result_snapshot_id` text,
	`result_workspace_version` integer,
	`journal_sequence` integer,
	`requested_at` text NOT NULL,
	`decision_at` text,
	`dispatched_at` text,
	`completed_at` text,
	`updated_at` text NOT NULL,
	CONSTRAINT "git_mutation_operations_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "git_mutation_operations_expected_snapshot_check" CHECK(length("expected_snapshot_id") = 64 AND "expected_snapshot_id" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "git_mutation_operations_result_snapshot_check" CHECK("result_snapshot_id" IS NULL OR (length("result_snapshot_id") = 64 AND "result_snapshot_id" NOT GLOB '*[^0-9a-f]*')),
	CONSTRAINT "git_mutation_operations_kind_check" CHECK("kind" IN ('stage', 'unstage')),
	CONSTRAINT "git_mutation_operations_lifecycle_check" CHECK("lifecycle" IN ('awaiting_approval', 'denied', 'approved', 'dispatching', 'succeeded', 'failed', 'ambiguous')),
	CONSTRAINT "git_mutation_operations_version_check" CHECK("expected_workspace_version" > 0 AND ("result_workspace_version" IS NULL OR "result_workspace_version" > 0))
);
--> statement-breakpoint
CREATE TABLE `git_workspace_projections` (
	`workspace_id` text PRIMARY KEY,
	`snapshot_id` text NOT NULL,
	`version` integer NOT NULL,
	`projection_json` text NOT NULL,
	`journal_sequence` integer NOT NULL,
	`observed_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "git_workspace_projections_snapshot_id_check" CHECK(length("snapshot_id") = 64 AND "snapshot_id" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "git_workspace_projections_version_check" CHECK("version" > 0),
	CONSTRAINT "git_workspace_projections_journal_sequence_check" CHECK("journal_sequence" > 0)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `git_mutation_operations_approval_unique` ON `git_mutation_operations` (`approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `git_mutation_operations_source_message_unique` ON `git_mutation_operations` (`source_message_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `git_mutation_operations_decision_message_unique` ON `git_mutation_operations` (`decision_message_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `git_mutation_operations_workspace_dispatch_unique` ON `git_mutation_operations` (`workspace_id`) WHERE "git_mutation_operations"."lifecycle" = 'dispatching';--> statement-breakpoint
CREATE INDEX `git_mutation_operations_thread_index` ON `git_mutation_operations` (`thread_id`);--> statement-breakpoint
CREATE INDEX `git_mutation_operations_workspace_index` ON `git_mutation_operations` (`workspace_id`,`lifecycle`);