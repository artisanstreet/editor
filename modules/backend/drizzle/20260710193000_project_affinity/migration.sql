ALTER TABLE `threads` ADD `affinity_version` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `primary_project_id` text;
--> statement-breakpoint
ALTER TABLE `threads` ADD `primary_project_json` text;
--> statement-breakpoint
ALTER TABLE `threads` ADD `linked_projects_json` text NOT NULL DEFAULT '[]';
--> statement-breakpoint
ALTER TABLE `threads` ADD `project_locked` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `project_affinity_scores_json` text NOT NULL DEFAULT '[]';
--> statement-breakpoint
ALTER TABLE `threads` ADD `rehome_suggestion_json` text;
--> statement-breakpoint
CREATE INDEX `threads_primary_project_index` ON `threads` (`primary_project_id`);
--> statement-breakpoint
CREATE TABLE `thread_project_affinity_evidence` (
	`basis_affinity_version` integer NOT NULL,
	`evidence_id` text PRIMARY KEY NOT NULL,
	`kind` text NOT NULL,
	`observed_at` text NOT NULL,
	`project_id` text NOT NULL,
	`project_json` text NOT NULL,
	`source_event_id` text NOT NULL,
	`source_journal_sequence` integer NOT NULL,
	`thread_id` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `thread_project_affinity_evidence_thread_index` ON `thread_project_affinity_evidence` (`thread_id`);
--> statement-breakpoint
CREATE UNIQUE INDEX `thread_project_affinity_evidence_source_unique` ON `thread_project_affinity_evidence` (`thread_id`, `source_event_id`, `kind`, `project_id`);
