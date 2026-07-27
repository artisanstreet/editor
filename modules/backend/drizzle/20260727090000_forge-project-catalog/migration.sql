CREATE TABLE `projects` (
	`project_id` text PRIMARY KEY NOT NULL,
	`display_name` text NOT NULL,
	`root_path` text NOT NULL,
	`attached_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `projects_root_path_unique` ON `projects` (`root_path`);
