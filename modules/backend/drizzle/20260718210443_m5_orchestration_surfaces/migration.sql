CREATE TABLE `workspace_conflicts` (
	`conflict_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL UNIQUE,
	`change_id` text NOT NULL,
	`attempting_thread_id` text NOT NULL,
	`attempting_run_id` text NOT NULL,
	`attempting_agent_id` text NOT NULL,
	`assignment_id` text,
	`group_id` text,
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`expected_identity_json` text NOT NULL,
	`observed_identity_json` text,
	`competing_change_id` text,
	`raw_origin_json` text,
	`resolution` text NOT NULL,
	`detected_at` text NOT NULL,
	CONSTRAINT "workspace_conflicts_resolution_check" CHECK("resolution" IN ('rejected', 'reconciled', 'user_action_required')),
	CONSTRAINT "workspace_conflicts_identity_check" CHECK(
				json_valid("expected_identity_json")
				AND json_extract("expected_identity_json", '$.algorithm') = 'sha256'
				AND json_type("expected_identity_json", '$.byte_count') = 'integer'
				AND json_extract("expected_identity_json", '$.byte_count') >= 0
				AND length(json_extract("expected_identity_json", '$.content_hash')) = 64
				AND json_extract("expected_identity_json", '$.content_hash') NOT GLOB '*[^0-9a-f]*'
				AND (
					"observed_identity_json" IS NULL
					OR (
						json_valid("observed_identity_json")
						AND json_extract("observed_identity_json", '$.algorithm') = 'sha256'
						AND json_type("observed_identity_json", '$.byte_count') = 'integer'
						AND json_extract("observed_identity_json", '$.byte_count') >= 0
						AND length(json_extract("observed_identity_json", '$.content_hash')) = 64
						AND json_extract("observed_identity_json", '$.content_hash') NOT GLOB '*[^0-9a-f]*'
					)
				)
			),
	CONSTRAINT "workspace_conflicts_raw_origin_check" CHECK("raw_origin_json" IS NULL OR json_valid("raw_origin_json"))
);
--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `reviewer_agent_id` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `reviewer_kind` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `reviewer_run_id` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `reviewer_assignment_id` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `reviewer_group_id` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `review_outcome` text;--> statement-breakpoint
ALTER TABLE `workspace_change_operations` ADD `review_comment` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `review_source_command_id` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_agent_id` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_kind` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_run_id` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_assignment_id` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_group_id` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `reviewer_raw_origin_json` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `review_outcome` text;--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `review_comment` text;--> statement-breakpoint
CREATE TEMP TABLE `__m5_workspace_mutation_authorities` AS SELECT * FROM `workspace_mutation_authorities`;--> statement-breakpoint
CREATE TEMP TABLE `__m5_workspace_mutation_payloads` AS SELECT * FROM `workspace_mutation_payloads`;--> statement-breakpoint
CREATE TEMP TABLE `__m5_workspace_change_diffs` AS SELECT * FROM `workspace_change_diffs`;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_workspace_change_operations` (
	`message_id` text PRIMARY KEY,
	`action` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`change_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`raw_origin_json` text,
	`reviewer_agent_id` text,
	`reviewer_kind` text,
	`reviewer_run_id` text,
	`reviewer_assignment_id` text,
	`reviewer_group_id` text,
	`review_outcome` text,
	`review_comment` text,
	`workspace_id` text,
	`path` text,
	`expected_identity_json` text,
	`result_identity_json` text,
	`diff_format_version` integer DEFAULT 1 NOT NULL,
	`lifecycle` text NOT NULL,
	`evidence_recorded` integer DEFAULT false NOT NULL,
	`journal_sequence` integer,
	`sent_at` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_change_operations_diff_format_version_check" CHECK("diff_format_version" = 1),
	CONSTRAINT "workspace_change_operations_reviewer_shape_check" CHECK(("reviewer_kind" IS NULL AND "reviewer_agent_id" IS NULL AND "reviewer_run_id" IS NULL AND "reviewer_assignment_id" IS NULL AND "reviewer_group_id" IS NULL) OR ("reviewer_kind" = 'user' AND "reviewer_agent_id" IS NULL AND "reviewer_run_id" IS NULL AND "reviewer_assignment_id" IS NULL AND "reviewer_group_id" IS NULL) OR ("reviewer_kind" = 'graph' AND "reviewer_agent_id" IS NOT NULL AND "reviewer_run_id" IS NOT NULL AND "reviewer_assignment_id" IS NOT NULL AND "reviewer_group_id" IS NOT NULL)),
	CONSTRAINT "workspace_change_operations_review_metadata_check" CHECK(("review_outcome" IS NULL OR "review_outcome" IN ('approved', 'changes_requested')) AND ("review_comment" IS NULL OR length("review_comment") <= 4096)),
	CONSTRAINT "workspace_change_operations_raw_origin_check" CHECK("raw_origin_json" IS NULL OR json_valid("raw_origin_json"))
);
--> statement-breakpoint
INSERT INTO `__new_workspace_change_operations`(`message_id`, `action`, `request_fingerprint`, `change_id`, `thread_id`, `run_id`, `agent_id`, `raw_origin_json`, `workspace_id`, `path`, `expected_identity_json`, `result_identity_json`, `diff_format_version`, `lifecycle`, `evidence_recorded`, `journal_sequence`, `sent_at`, `created_at`, `updated_at`) SELECT `message_id`, `action`, `request_fingerprint`, `change_id`, `thread_id`, `run_id`, `agent_id`, `raw_origin_json`, `workspace_id`, `path`, `expected_identity_json`, `result_identity_json`, `diff_format_version`, `lifecycle`, `evidence_recorded`, `journal_sequence`, `sent_at`, `created_at`, `updated_at` FROM `workspace_change_operations`;--> statement-breakpoint
DROP TABLE `workspace_change_operations`;--> statement-breakpoint
ALTER TABLE `__new_workspace_change_operations` RENAME TO `workspace_change_operations`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_workspace_changes` (
	`change_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`before_identity_json` text NOT NULL,
	`after_identity_json` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`raw_origin_json` text,
	`review_state` text NOT NULL,
	`rollback_state` text NOT NULL,
	`reviewed_at` text,
	`review_source_command_id` text,
	`reviewer_agent_id` text,
	`reviewer_kind` text,
	`reviewer_run_id` text,
	`reviewer_assignment_id` text,
	`reviewer_group_id` text,
	`reviewer_raw_origin_json` text,
	`review_outcome` text,
	`review_comment` text,
	`rolled_back_at` text,
	`version` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`diff_state` text DEFAULT 'legacy_unavailable' NOT NULL,
	CONSTRAINT "workspace_changes_diff_state_check" CHECK("diff_state" IN ('available', 'legacy_unavailable')),
	CONSTRAINT "workspace_changes_reviewer_shape_check" CHECK(("reviewer_kind" IS NULL AND "reviewer_agent_id" IS NULL AND "reviewer_run_id" IS NULL AND "reviewer_assignment_id" IS NULL AND "reviewer_group_id" IS NULL) OR ("reviewer_kind" = 'user' AND "reviewer_agent_id" IS NULL AND "reviewer_run_id" IS NULL AND "reviewer_assignment_id" IS NULL AND "reviewer_group_id" IS NULL) OR ("reviewer_kind" = 'graph' AND "reviewer_agent_id" IS NOT NULL AND "reviewer_run_id" IS NOT NULL AND "reviewer_assignment_id" IS NOT NULL AND "reviewer_group_id" IS NOT NULL)),
	CONSTRAINT "workspace_changes_review_metadata_check" CHECK(("review_outcome" IS NULL OR "review_outcome" IN ('approved', 'changes_requested')) AND ("review_comment" IS NULL OR length("review_comment") <= 4096)),
	CONSTRAINT "workspace_changes_reviewer_raw_origin_check" CHECK("reviewer_raw_origin_json" IS NULL OR json_valid("reviewer_raw_origin_json"))
);
--> statement-breakpoint
INSERT INTO `__new_workspace_changes`(`change_id`, `source_command_id`, `thread_id`, `workspace_id`, `path`, `before_identity_json`, `after_identity_json`, `run_id`, `agent_id`, `raw_origin_json`, `review_state`, `rollback_state`, `reviewed_at`, `rolled_back_at`, `version`, `created_at`, `updated_at`, `diff_state`) SELECT `change_id`, `source_command_id`, `thread_id`, `workspace_id`, `path`, `before_identity_json`, `after_identity_json`, `run_id`, `agent_id`, `raw_origin_json`, `review_state`, `rollback_state`, `reviewed_at`, `rolled_back_at`, `version`, `created_at`, `updated_at`, `diff_state` FROM `workspace_changes`;--> statement-breakpoint
DROP TABLE `workspace_changes`;--> statement-breakpoint
ALTER TABLE `__new_workspace_changes` RENAME TO `workspace_changes`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
INSERT INTO `workspace_mutation_authorities` SELECT * FROM `__m5_workspace_mutation_authorities`;--> statement-breakpoint
INSERT INTO `workspace_mutation_payloads` SELECT * FROM `__m5_workspace_mutation_payloads`;--> statement-breakpoint
INSERT INTO `workspace_change_diffs` SELECT * FROM `__m5_workspace_change_diffs`;--> statement-breakpoint
DROP TABLE `__m5_workspace_mutation_authorities`;--> statement-breakpoint
DROP TABLE `__m5_workspace_mutation_payloads`;--> statement-breakpoint
DROP TABLE `__m5_workspace_change_diffs`;--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_change_operations_change_action_unique` ON `workspace_change_operations` (`change_id`,`action`);--> statement-breakpoint
CREATE INDEX `workspace_change_operations_change_id_index` ON `workspace_change_operations` (`change_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_changes_source_command_unique` ON `workspace_changes` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `workspace_changes_thread_id_index` ON `workspace_changes` (`thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_changes_thread_workspace_index` ON `workspace_changes` (`thread_id`,`workspace_id`);--> statement-breakpoint
CREATE INDEX `workspace_conflicts_thread_index` ON `workspace_conflicts` (`attempting_thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_conflicts_change_index` ON `workspace_conflicts` (`change_id`);
