CREATE TABLE `project_identities` (
	`created_at` text NOT NULL,
	`project_id` text NOT NULL,
	`root_path` text PRIMARY KEY
);
--> statement-breakpoint
CREATE UNIQUE INDEX `project_identities_project_id_unique` ON `project_identities` (`project_id`);