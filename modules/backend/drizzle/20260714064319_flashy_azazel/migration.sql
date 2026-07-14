CREATE TABLE `project_hosted_origins` (
	`project_id` text PRIMARY KEY,
	`provider_id` text NOT NULL,
	`canonical_host` text NOT NULL,
	`owner` text NOT NULL,
	`name` text NOT NULL,
	`native_id` text NOT NULL,
	`selected_account_login` text NOT NULL,
	`clone_url` text NOT NULL,
	`web_url` text NOT NULL,
	`remote_name` text NOT NULL,
	`fetch_url` text NOT NULL,
	`push_url` text NOT NULL,
	CONSTRAINT `fk_project_hosted_origins_project_id_projects_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `projects`(`project_id`) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE TABLE `projects` (
	`project_id` text PRIMARY KEY,
	`workspace_id` text NOT NULL,
	`canonical_root` text NOT NULL,
	`display_name` text NOT NULL,
	`registered_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `project_hosted_origins_native_identity_unique` ON `project_hosted_origins` (`provider_id`,`canonical_host`,`native_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `project_hosted_origins_coordinate_unique` ON `project_hosted_origins` (`provider_id`,`canonical_host`,`owner`,`name`);--> statement-breakpoint
CREATE INDEX `project_hosted_origins_project_id_index` ON `project_hosted_origins` (`project_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `projects_workspace_id_unique` ON `projects` (`workspace_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `projects_canonical_root_unique` ON `projects` (`canonical_root`);--> statement-breakpoint
CREATE INDEX `projects_registered_at_index` ON `projects` (`registered_at`);