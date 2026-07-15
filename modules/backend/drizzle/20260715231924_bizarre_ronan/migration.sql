CREATE TABLE `hosted_git_mutation_approvals` (
	`approval_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`snapshot_version` integer NOT NULL,
	`expected_head_commit` text NOT NULL,
	`pull_request_number` integer NOT NULL,
	`pull_request_origin_json` text NOT NULL,
	`repository_json` text NOT NULL,
	`selection_json` text NOT NULL,
	`operation_summary_json` text NOT NULL,
	`state` text NOT NULL,
	`decision_message_id` text,
	`approved` integer,
	`decided_at` text,
	`execution_started_at` text,
	`result_json` text,
	`rejection_reason` text,
	`unknown_reason` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "hosted_git_mutation_approvals_fingerprint_check" CHECK(
				length("request_fingerprint") = 64
				AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "hosted_git_mutation_approvals_target_check" CHECK("snapshot_version" >= 1 AND "pull_request_number" >= 1),
	CONSTRAINT "hosted_git_mutation_approvals_state_check" CHECK(
				"state" IN (
					'requested', 'approved', 'executing', 'applied',
					'rejected', 'outcome_unknown', 'denied'
				)
			),
	CONSTRAINT "hosted_git_mutation_approvals_decision_check" CHECK(
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
					"state" = 'executing'
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
				)
				OR (
					"state" = 'rejected'
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
				)
				OR (
					"state" IN ('applied', 'outcome_unknown')
					AND "decision_message_id" IS NOT NULL
					AND "approved" = 1
					AND "decided_at" IS NOT NULL
					AND "execution_started_at" IS NOT NULL
				)
			),
	CONSTRAINT "hosted_git_mutation_approvals_outcome_check" CHECK(
				(
					"state" IN ('requested', 'approved', 'executing', 'denied')
					AND "result_json" IS NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'applied'
					AND "result_json" IS NOT NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'rejected'
					AND "result_json" IS NULL
					AND "rejection_reason" IS NOT NULL
					AND "unknown_reason" IS NULL
				)
				OR (
					"state" = 'outcome_unknown'
					AND "result_json" IS NULL
					AND "rejection_reason" IS NULL
					AND "unknown_reason" IS NOT NULL
				)
			)
);
--> statement-breakpoint
CREATE TABLE `hosted_git_mutation_artifacts` (
	`approval_id` text PRIMARY KEY,
	`operation_json` text,
	`operation_binding` text NOT NULL,
	`provider_result_json` text,
	`selection_json` text,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_hosted_git_mutation_artifacts_approval_id_hosted_git_mutation_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `hosted_git_mutation_approvals`(`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_git_mutation_artifacts_binding_check" CHECK(
				length("operation_binding") = 64
				AND "operation_binding" NOT GLOB '*[^0-9a-f]*'
			),
	CONSTRAINT "hosted_git_mutation_artifacts_private_pair_check" CHECK(
				(
					"operation_json" IS NULL
					AND "selection_json" IS NULL
				)
				OR (
					"operation_json" IS NOT NULL
					AND "selection_json" IS NOT NULL
				)
			)
);
--> statement-breakpoint
CREATE TABLE `hosted_git_mutation_claims` (
	`workspace_id` text PRIMARY KEY,
	`approval_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`claim_token` text NOT NULL,
	`owner_instance_id` text DEFAULT 'unowned' NOT NULL,
	`claimed_at` text NOT NULL,
	`lease_expires_at` text NOT NULL,
	`execution_started_at` text,
	`execution_completed_at` text,
	CONSTRAINT `fk_hosted_git_mutation_claims_approval_id_hosted_git_mutation_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `hosted_git_mutation_approvals`(`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_git_mutation_claims_execution_pair_check" CHECK("execution_completed_at" IS NULL OR "execution_started_at" IS NOT NULL)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_git_mutation_approvals_source_command_unique` ON `hosted_git_mutation_approvals` (`source_command_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_git_mutation_approvals_decision_message_unique` ON `hosted_git_mutation_approvals` (`decision_message_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_mutation_approvals_thread_index` ON `hosted_git_mutation_approvals` (`thread_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_mutation_approvals_workspace_index` ON `hosted_git_mutation_approvals` (`workspace_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_mutation_approvals_state_index` ON `hosted_git_mutation_approvals` (`state`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_git_mutation_claims_approval_unique` ON `hosted_git_mutation_claims` (`approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_git_mutation_claims_token_unique` ON `hosted_git_mutation_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `hosted_git_mutation_claims_thread_index` ON `hosted_git_mutation_claims` (`thread_id`);--> statement-breakpoint
CREATE INDEX `hosted_git_mutation_claims_lease_index` ON `hosted_git_mutation_claims` (`lease_expires_at`);
