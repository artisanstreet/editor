CREATE TABLE `workspace_mutation_authorities` (
	`message_id` text PRIMARY KEY,
	`change_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`authority_kind` text NOT NULL,
	`working_directory` text NOT NULL,
	`group_id` text,
	`assignment_id` text,
	`scope_kind` text,
	`scope_value` text,
	`approval` text,
	`created_at` text NOT NULL,
	CONSTRAINT `fk_workspace_mutation_authorities_message_id_workspace_change_operations_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `workspace_change_operations`(`message_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_mutation_authorities_shape_check" CHECK(
				(
					"authority_kind" = 'base_run'
					AND "group_id" IS NULL
					AND "assignment_id" IS NULL
					AND "scope_kind" IS NULL
					AND "scope_value" IS NULL
					AND "approval" IS NULL
				)
				OR (
					"authority_kind" = 'graph_run'
					AND "group_id" IS NOT NULL
					AND "assignment_id" IS NOT NULL
					AND "scope_kind" IN ('repo', 'files')
					AND "scope_value" IS NOT NULL
					AND "approval" IN ('never', 'on_request', 'always')
				)
			)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_mutation_authorities_change_id_unique` ON `workspace_mutation_authorities` (`change_id`);--> statement-breakpoint
CREATE INDEX `workspace_mutation_authorities_thread_id_index` ON `workspace_mutation_authorities` (`thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_mutation_authorities_run_id_index` ON `workspace_mutation_authorities` (`run_id`);