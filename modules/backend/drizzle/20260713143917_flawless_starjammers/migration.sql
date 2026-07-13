CREATE TABLE `workspace_git_changed_files` (
	`workspace_id` text NOT NULL,
	`path` text NOT NULL,
	`original_path` text,
	`status` text NOT NULL,
	`staged` integer NOT NULL,
	`unstaged` integer NOT NULL,
	`untracked` integer NOT NULL,
	`conflicted` integer NOT NULL,
	`version` integer NOT NULL,
	CONSTRAINT `workspace_git_changed_files_pk` PRIMARY KEY(`workspace_id`, `path`),
	CONSTRAINT `fk_workspace_git_changed_files_workspace_id_workspace_git_sessions_workspace_id_fk` FOREIGN KEY (`workspace_id`) REFERENCES `workspace_git_sessions`(`workspace_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_git_changed_files_version_check" CHECK("version" >= 1)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_checkout_approvals` (
	`approval_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`expected_session_version` integer NOT NULL,
	`source_branch` text NOT NULL,
	`source_head` text NOT NULL,
	`target_branch` text NOT NULL,
	`target_head` text NOT NULL,
	`state` text NOT NULL,
	`decision_message_id` text,
	`approved` integer,
	`decided_at` text,
	`execution_started_at` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_checkout_approvals_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_git_checkout_approvals_state_check" CHECK("state" IN ('requested', 'approved', 'executing', 'applied', 'denied', 'rejected', 'unknown')),
	CONSTRAINT "workspace_git_checkout_approvals_decision_check" CHECK(
				(
					"state" = 'requested'
					AND "decision_message_id" IS NULL
					AND "approved" IS NULL
					AND "decided_at" IS NULL
					AND "execution_started_at" IS NULL
				)
				OR (
					"state" = 'denied'
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 0
					AND "decided_at" IS NOT NULL
					AND "execution_started_at" IS NULL
				)
				OR (
					"state" = 'approved'
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
					AND "execution_started_at" IS NULL
				)
				OR (
					"state" IN ('executing', 'applied', 'rejected', 'unknown')
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
					AND "execution_started_at" IS NOT NULL
				)
			),
	CONSTRAINT "workspace_git_checkout_approvals_version_check" CHECK("expected_session_version" >= 1)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_checkout_claims` (
	`workspace_id` text PRIMARY KEY,
	`approval_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`claimed_at` text NOT NULL,
	CONSTRAINT `fk_workspace_git_checkout_claims_approval_id_workspace_git_checkout_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `workspace_git_checkout_approvals`(`approval_id`) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE TABLE `workspace_git_operations` (
	`operation_id` text PRIMARY KEY,
	`source_command_id` text,
	`request_fingerprint` text NOT NULL,
	`kind` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`session_version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	`evidence_recorded` integer DEFAULT false NOT NULL,
	`evidence_root_path` text,
	`evidence_worktree_path` text,
	`evidence_branch` text,
	`evidence_changed_file_count` integer,
	`evidence_has_diff` integer,
	`sent_at` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_operations_kind_check" CHECK("kind" IN ('refresh', 'checkout', 'recovery')),
	CONSTRAINT "workspace_git_operations_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_git_operations_version_sequence_check" CHECK("session_version" >= 1 AND "journal_sequence" >= 1),
	CONSTRAINT "workspace_git_operations_evidence_check" CHECK(
				(
					"evidence_recorded" = 1
				)
				OR (
					"evidence_root_path" IS NOT NULL
					AND "evidence_worktree_path" IS NOT NULL
					AND "evidence_changed_file_count" IS NOT NULL
					AND "evidence_changed_file_count" >= 0
					AND "evidence_has_diff" IS NOT NULL
				)
			)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_sessions` (
	`workspace_id` text PRIMARY KEY,
	`repository_root` text,
	`selected_worktree_path` text,
	`state` text NOT NULL,
	`blockers_json` text NOT NULL,
	`branch` text,
	`head` text,
	`additions` integer NOT NULL,
	`deletions` integer NOT NULL,
	`files` integer NOT NULL,
	`has_diff` integer NOT NULL,
	`version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	`observed_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_sessions_state_check" CHECK("state" IN ('ready', 'blocked', 'unavailable')),
	CONSTRAINT "workspace_git_sessions_counts_check" CHECK(
				"additions" >= 0
				AND "deletions" >= 0
				AND "files" >= 0
				AND "version" >= 1
				AND "journal_sequence" >= 1
			),
	CONSTRAINT "workspace_git_sessions_repository_shape_check" CHECK(
				(
					"state" = 'unavailable'
					AND "repository_root" IS NULL
					AND "selected_worktree_path" IS NULL
				)
				OR (
					"state" IN ('ready', 'blocked')
					AND "repository_root" IS NOT NULL
					AND "selected_worktree_path" IS NOT NULL
				)
			)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_worktrees` (
	`workspace_id` text NOT NULL,
	`ordinal` integer NOT NULL,
	`adapter_path` text NOT NULL,
	`location` text NOT NULL,
	`branch` text,
	`head` text,
	`bare` integer NOT NULL,
	`detached` integer NOT NULL,
	`locked` integer NOT NULL,
	`prunable` integer NOT NULL,
	`version` integer NOT NULL,
	CONSTRAINT `workspace_git_worktrees_pk` PRIMARY KEY(`workspace_id`, `ordinal`),
	CONSTRAINT `fk_workspace_git_worktrees_workspace_id_workspace_git_sessions_workspace_id_fk` FOREIGN KEY (`workspace_id`) REFERENCES `workspace_git_sessions`(`workspace_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_git_worktrees_location_check" CHECK("location" IN ('selected', 'external')),
	CONSTRAINT "workspace_git_worktrees_ordinal_version_check" CHECK("ordinal" >= 0 AND "version" >= 1)
);
--> statement-breakpoint
CREATE INDEX `workspace_git_changed_files_workspace_index` ON `workspace_git_changed_files` (`workspace_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_checkout_approvals_source_command_unique` ON `workspace_git_checkout_approvals` (`source_command_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_checkout_approvals_decision_message_unique` ON `workspace_git_checkout_approvals` (`decision_message_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_checkout_approvals_thread_index` ON `workspace_git_checkout_approvals` (`thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_checkout_approvals_state_index` ON `workspace_git_checkout_approvals` (`state`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_checkout_claims_approval_unique` ON `workspace_git_checkout_claims` (`approval_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_checkout_claims_thread_index` ON `workspace_git_checkout_claims` (`thread_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_operations_source_command_unique` ON `workspace_git_operations` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_operations_workspace_index` ON `workspace_git_operations` (`workspace_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_operations_pending_evidence_index` ON `workspace_git_operations` (`evidence_recorded`);--> statement-breakpoint
CREATE INDEX `workspace_git_worktrees_workspace_index` ON `workspace_git_worktrees` (`workspace_id`);