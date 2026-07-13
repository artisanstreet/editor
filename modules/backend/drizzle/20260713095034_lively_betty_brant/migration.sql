ALTER TABLE `workspace_change_operations` ADD `diff_format_version` integer DEFAULT 1 NOT NULL CONSTRAINT "workspace_change_operations_diff_format_version_check" CHECK("diff_format_version" = 1);--> statement-breakpoint
ALTER TABLE `workspace_changes` ADD `diff_state` text DEFAULT 'legacy_unavailable' NOT NULL CONSTRAINT "workspace_changes_diff_state_check" CHECK("diff_state" IN ('available', 'legacy_unavailable'));--> statement-breakpoint
CREATE TABLE `workspace_change_diffs` (
	`change_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`before_identity_json` text NOT NULL,
	`after_identity_json` text NOT NULL,
	`format` text NOT NULL,
	`format_version` integer NOT NULL,
	`context_lines` integer NOT NULL,
	`patch` blob NOT NULL,
	`patch_byte_count` integer NOT NULL,
	`patch_hash` text NOT NULL,
	`added_line_count` integer NOT NULL,
	`removed_line_count` integer NOT NULL,
	`created_at` text NOT NULL,
	CONSTRAINT `fk_workspace_change_diffs_change_id_workspace_changes_change_id_fk` FOREIGN KEY (`change_id`) REFERENCES `workspace_changes`(`change_id`) ON DELETE CASCADE,
	CONSTRAINT `fk_workspace_change_diffs_source_command_id_workspace_change_operations_message_id_fk` FOREIGN KEY (`source_command_id`) REFERENCES `workspace_change_operations`(`message_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_change_diffs_format_check" CHECK("format" = 'unified'),
	CONSTRAINT "workspace_change_diffs_format_version_check" CHECK("format_version" = 1),
	CONSTRAINT "workspace_change_diffs_context_check" CHECK("context_lines" = 3),
	CONSTRAINT "workspace_change_diffs_patch_check" CHECK(
				length("patch") = "patch_byte_count"
				AND "patch_byte_count" BETWEEN 0 AND 16777216
				AND length("patch_hash") = 64
				AND "patch_hash" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_change_diffs_line_count_check" CHECK(
				"added_line_count" BETWEEN 0 AND 100000
				AND "removed_line_count" BETWEEN 0 AND 100000
			)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_change_diffs_source_command_unique` ON `workspace_change_diffs` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `workspace_change_diffs_thread_id_index` ON `workspace_change_diffs` (`thread_id`);
