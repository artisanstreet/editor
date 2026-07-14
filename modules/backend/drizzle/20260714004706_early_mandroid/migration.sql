CREATE TABLE `workspace_git_mutation_approvals` (
	`approval_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`expected_session_version` integer NOT NULL,
	`action_approval_id` text,
	`operation_summary_json` text NOT NULL,
	`source_branch` text,
	`source_head` text NOT NULL,
	`state` text NOT NULL,
	`decision_message_id` text,
	`approved` integer,
	`decided_at` text,
	`execution_started_at` text,
	`resulting_branch` text,
	`resulting_head` text,
	`remote_head` text,
	`required_action` text,
	`rejection_reason` text,
	`unknown_reason` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_git_mutation_approvals_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_git_mutation_approvals_version_check" CHECK("expected_session_version" >= 1),
	CONSTRAINT "workspace_git_mutation_approvals_state_check" CHECK(
				"state" IN (
					'requested', 'approved', 'executing', 'applied',
					'action_required', 'rejected', 'outcome_unknown', 'denied'
				)
			),
	CONSTRAINT "workspace_git_mutation_approvals_decision_check" CHECK(
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
					"state" IN (
						'executing', 'applied', 'action_required', 'rejected', 'outcome_unknown'
					)
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
					AND "execution_started_at" IS NOT NULL
				)
			),
	CONSTRAINT "workspace_git_mutation_approvals_outcome_check" CHECK(
				(
					"state" IN ('requested', 'approved', 'executing', 'denied')
					AND "resulting_branch" IS NULL
					AND "resulting_head" IS NULL
					AND "remote_head" IS NULL
					AND "required_action" IS NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'applied'
					AND "resulting_head" IS NOT NULL
					AND "required_action" IS NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'action_required'
					AND "required_action" IS NOT NULL
					AND "resulting_branch" IS NULL
					AND "resulting_head" IS NULL
					AND "remote_head" IS NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'rejected'
					AND "rejection_reason" IS NOT NULL
					AND "resulting_branch" IS NULL
					AND "resulting_head" IS NULL
					AND "remote_head" IS NULL
					AND "required_action" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'outcome_unknown'
					AND "unknown_reason" IS NOT NULL
					AND "resulting_branch" IS NULL
					AND "resulting_head" IS NULL
					AND "remote_head" IS NULL
					AND "required_action" IS NULL
					AND "rejection_reason" IS NULL
				)
			)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_mutation_artifacts` (
	`approval_id` text PRIMARY KEY,
	`operation_json` text NOT NULL,
	`plan_json` text NOT NULL,
	`plan_binding` text NOT NULL,
	`attempt_json` text,
	`attempt_binding` text,
	`reconciliation_json` text,
	`reconciled_at` text,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_workspace_git_mutation_artifacts_approval_id_workspace_git_mutation_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `workspace_git_mutation_approvals`(`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_git_mutation_artifacts_plan_binding_check" CHECK(
				length("plan_binding") = 64
				AND "plan_binding" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "workspace_git_mutation_artifacts_attempt_binding_check" CHECK(
				(
					"attempt_json" IS NULL
					AND "attempt_binding" IS NULL
				)
				OR (
					"attempt_json" IS NOT NULL
					AND length("attempt_binding") = 64
					AND "attempt_binding" NOT GLOB '*[^0-9a-f]*'
				)
			),
	CONSTRAINT "workspace_git_mutation_artifacts_reconciliation_pair_check" CHECK(
				("reconciliation_json" IS NULL) = ("reconciled_at" IS NULL)
			)
);
--> statement-breakpoint
CREATE TABLE `workspace_git_mutation_claims` (
	`workspace_id` text PRIMARY KEY,
	`approval_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`claim_token` text NOT NULL,
	`claimed_at` text NOT NULL,
	CONSTRAINT `fk_workspace_git_mutation_claims_approval_id_workspace_git_mutation_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `workspace_git_mutation_approvals`(`approval_id`) ON DELETE CASCADE
);
--> statement-breakpoint
PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_workspace_git_operations` (
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
	CONSTRAINT "workspace_git_operations_kind_check" CHECK("kind" IN ('refresh', 'checkout', 'recovery', 'mutation')),
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
INSERT INTO `__new_workspace_git_operations`(`operation_id`, `source_command_id`, `request_fingerprint`, `kind`, `thread_id`, `workspace_id`, `session_version`, `journal_sequence`, `evidence_recorded`, `evidence_root_path`, `evidence_worktree_path`, `evidence_branch`, `evidence_changed_file_count`, `evidence_has_diff`, `sent_at`, `created_at`, `updated_at`) SELECT `operation_id`, `source_command_id`, `request_fingerprint`, `kind`, `thread_id`, `workspace_id`, `session_version`, `journal_sequence`, `evidence_recorded`, `evidence_root_path`, `evidence_worktree_path`, `evidence_branch`, `evidence_changed_file_count`, `evidence_has_diff`, `sent_at`, `created_at`, `updated_at` FROM `workspace_git_operations`;--> statement-breakpoint
DROP TABLE `workspace_git_operations`;--> statement-breakpoint
ALTER TABLE `__new_workspace_git_operations` RENAME TO `workspace_git_operations`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_operations_source_command_unique` ON `workspace_git_operations` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_operations_workspace_index` ON `workspace_git_operations` (`workspace_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_operations_pending_evidence_index` ON `workspace_git_operations` (`evidence_recorded`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_mutation_approvals_source_command_unique` ON `workspace_git_mutation_approvals` (`source_command_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_mutation_approvals_decision_message_unique` ON `workspace_git_mutation_approvals` (`decision_message_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_mutation_approvals_thread_index` ON `workspace_git_mutation_approvals` (`thread_id`);--> statement-breakpoint
CREATE INDEX `workspace_git_mutation_approvals_state_index` ON `workspace_git_mutation_approvals` (`state`);--> statement-breakpoint
CREATE INDEX `workspace_git_mutation_approvals_action_parent_index` ON `workspace_git_mutation_approvals` (`action_approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_mutation_claims_approval_unique` ON `workspace_git_mutation_claims` (`approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `workspace_git_mutation_claims_claim_token_unique` ON `workspace_git_mutation_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `workspace_git_mutation_claims_thread_index` ON `workspace_git_mutation_claims` (`thread_id`);