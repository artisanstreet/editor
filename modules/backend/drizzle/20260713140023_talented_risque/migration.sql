CREATE TABLE `workspace_replace_approvals` (
	`approval_id` text PRIMARY KEY,
	`message_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`operation_sent_at` text NOT NULL,
	`change_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`before_identity_json` text NOT NULL,
	`after_identity_json` text NOT NULL,
	`policy` text NOT NULL,
	`reason` text NOT NULL,
	`state` text NOT NULL,
	`decision_message_id` text,
	`approved` integer,
	`decided_at` text,
	`raw_origin_json` text,
	`format` text NOT NULL,
	`format_version` integer NOT NULL,
	`context_lines` integer NOT NULL,
	`patch` blob NOT NULL,
	`patch_byte_count` integer NOT NULL,
	`patch_hash` text NOT NULL,
	`added_line_count` integer NOT NULL,
	`removed_line_count` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_workspace_replace_approvals_message_id_workspace_change_operations_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `workspace_change_operations`(`message_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_replace_approvals_policy_check" CHECK("policy" IN ('on_request', 'always')),
	CONSTRAINT "workspace_replace_approvals_request_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_replace_approvals_state_check" CHECK("state" IN ('requested', 'approved', 'executing', 'denied', 'applied', 'rejected')),
	CONSTRAINT "workspace_replace_approvals_decision_check" CHECK(
				(
					"state" = 'requested'
					AND "decision_message_id" IS NULL
					AND "approved" IS NULL
					AND "decided_at" IS NULL
				)
				OR (
					"state" = 'denied'
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 0
					AND "decided_at" IS NOT NULL
				)
				OR (
					"state" IN ('approved', 'executing', 'applied', 'rejected')
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
				)
			),
	CONSTRAINT "workspace_replace_approvals_format_check" CHECK("format" = 'unified'),
	CONSTRAINT "workspace_replace_approvals_format_version_check" CHECK("format_version" = 1),
	CONSTRAINT "workspace_replace_approvals_context_check" CHECK("context_lines" = 3),
	CONSTRAINT "workspace_replace_approvals_patch_check" CHECK(
				length("patch") = "patch_byte_count"
				AND "patch_byte_count" BETWEEN 0 AND 16777216
				AND length("patch_hash") = 64
				AND "patch_hash" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_replace_approvals_line_count_check" CHECK(
				"added_line_count" BETWEEN 0 AND 100000
				AND "removed_line_count" BETWEEN 0 AND 100000
			)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_replace_approvals_message_id_unique` ON `workspace_replace_approvals` (`message_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_replace_approvals_change_id_unique` ON `workspace_replace_approvals` (`change_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_replace_approvals_decision_message_unique` ON `workspace_replace_approvals` (`decision_message_id`);--> statement-breakpoint
CREATE INDEX `workspace_replace_approvals_thread_id_index` ON `workspace_replace_approvals` (`thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_replace_approvals_state_index` ON `workspace_replace_approvals` (`state`);