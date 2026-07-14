CREATE TABLE `workspace_git_fetch_operations` (
	`message_id` text PRIMARY KEY,
	`kind` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`sent_at` text NOT NULL,
	`enabled` integer,
	`thread_id` text,
	`workspace_id` text,
	`attempt_id` text,
	`status` text NOT NULL,
	`result` text,
	`attempted_at` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_fetch_operations_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_git_fetch_operations_shape_check" CHECK(
				(
					"kind" = 'policy'
					AND "enabled" IS NOT NULL
					AND "thread_id" IS NULL
					AND "workspace_id" IS NULL
					AND "attempt_id" IS NULL
					AND "status" = 'terminal'
					AND "result" IS NULL
					AND "attempted_at" IS NULL
				)
				OR (
					"kind" = 'manual'
					AND "enabled" IS NULL
					AND "thread_id" IS NOT NULL
					AND "workspace_id" IS NOT NULL
					AND "attempt_id" IS NOT NULL
					AND (
						("status" = 'pending' AND "result" IS NULL AND "attempted_at" IS NULL)
						OR (
							"status" = 'terminal'
							AND "result" IN ('succeeded', 'failed', 'unavailable')
							AND "attempted_at" IS NOT NULL
						)
					)
				)
			)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_fetch_policies` (
	`policy_id` integer PRIMARY KEY,
	`enabled` integer DEFAULT false NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_fetch_policies_singleton_check" CHECK("policy_id" = 1)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_fetch_states` (
	`workspace_id` text PRIMARY KEY,
	`last_attempted_at` text,
	`last_result` text,
	`version` integer DEFAULT 0 NOT NULL,
	`active_attempt_id` text,
	`active_kind` text,
	`active_message_id` text,
	`started_at` text,
	`lease_owner` text,
	`lease_expires_at` text,
	CONSTRAINT "workspace_git_fetch_states_result_check" CHECK(
				("last_attempted_at" IS NULL) = ("last_result" IS NULL)
				AND (
					"last_result" IS NULL
					OR "last_result" IN ('succeeded', 'failed', 'unavailable')
				)
			),
	CONSTRAINT "workspace_git_fetch_states_version_check" CHECK("version" >= 0),
	CONSTRAINT "workspace_git_fetch_states_active_check" CHECK(
				(
					"active_attempt_id" IS NULL
					AND "active_kind" IS NULL
					AND "active_message_id" IS NULL
					AND "started_at" IS NULL
					AND "lease_owner" IS NULL
					AND "lease_expires_at" IS NULL
				)
				OR (
					"active_attempt_id" IS NOT NULL
					AND "active_kind" = 'automatic'
					AND "active_message_id" IS NULL
					AND "started_at" IS NOT NULL
					AND "lease_owner" IS NOT NULL
					AND "lease_expires_at" IS NOT NULL
				)
				OR (
					"active_attempt_id" IS NOT NULL
					AND "active_kind" = 'manual'
					AND "active_message_id" IS NOT NULL
					AND "started_at" IS NOT NULL
					AND "lease_owner" IS NOT NULL
					AND "lease_expires_at" IS NOT NULL
				)
			)
);
--> statement-breakpoint
CREATE INDEX `workspace_git_fetch_operations_pending_index` ON `workspace_git_fetch_operations` (`workspace_id`,`status`,`created_at`);
--> statement-breakpoint
INSERT INTO `workspace_git_fetch_policies` (`policy_id`, `enabled`, `updated_at`)
VALUES (1, 0, '1970-01-01T00:00:00.000Z');
