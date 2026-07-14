CREATE TABLE `hosted_git_snapshot_operations` (
	`operation_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`snapshot_version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	`sent_at` text NOT NULL,
	CONSTRAINT `fk_hosted_git_snapshot_operations_project_id_projects_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `projects`(`project_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_git_snapshot_operations_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "hosted_git_snapshot_operations_version_check" CHECK("snapshot_version" >= 1),
	CONSTRAINT "hosted_git_snapshot_operations_journal_sequence_check" CHECK("journal_sequence" >= 1)
);
--> statement-breakpoint
CREATE TABLE `hosted_git_snapshots` (
	`project_id` text PRIMARY KEY,
	`lookup_json` text NOT NULL,
	`observed_at` text NOT NULL,
	`version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	CONSTRAINT `fk_hosted_git_snapshots_project_id_projects_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `projects`(`project_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_git_snapshots_version_check" CHECK("version" >= 1),
	CONSTRAINT "hosted_git_snapshots_journal_sequence_check" CHECK("journal_sequence" >= 1)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_git_snapshot_operations_source_command_unique` ON `hosted_git_snapshot_operations` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_snapshot_operations_thread_index` ON `hosted_git_snapshot_operations` (`thread_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_snapshot_operations_project_index` ON `hosted_git_snapshot_operations` (`project_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_snapshots_journal_sequence_index` ON `hosted_git_snapshots` (`journal_sequence`);