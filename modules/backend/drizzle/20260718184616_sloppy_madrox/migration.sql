CREATE TABLE `legacy_workspace_change_projections` (
	`change_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`thread_id` text NOT NULL
);
--> statement-breakpoint
INSERT INTO `legacy_workspace_change_projections` (`change_id`, `source_command_id`, `thread_id`)
SELECT `change_id`, `source_command_id`, `thread_id`
FROM `workspace_changes`
WHERE `diff_state` = 'legacy_unavailable';
