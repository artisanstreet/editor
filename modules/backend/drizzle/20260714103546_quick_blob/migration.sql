CREATE TABLE `hosted_project_clone_approvals` (
	`approval_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`thread_id` text NOT NULL,
	`destination_path` text NOT NULL,
	`repository_json` text NOT NULL,
	`state` text NOT NULL,
	`decision_message_id` text,
	`approved` integer,
	`decided_at` text,
	`execution_started_at` text,
	`project_json` text,
	`attachment` text,
	`rejection_reason` text,
	`unknown_reason` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "hosted_project_clone_approvals_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "hosted_project_clone_approvals_state_check" CHECK("state" IN ('requested', 'reused', 'approved', 'executing', 'applied', 'attachment_conflict', 'rejected', 'outcome_unknown', 'denied')),
	CONSTRAINT "hosted_project_clone_approvals_decision_check" CHECK(
			("state" IN ('requested', 'reused') AND "decision_message_id" IS NULL AND "approved" IS NULL AND "decided_at" IS NULL AND "execution_started_at" IS NULL)
			OR ("state" = 'denied' AND "decision_message_id" IS NOT NULL AND "approved" = 0 AND "decided_at" IS NOT NULL AND "execution_started_at" IS NULL)
			OR ("state" = 'approved' AND "decision_message_id" IS NOT NULL AND "approved" = 1 AND "decided_at" IS NOT NULL AND "execution_started_at" IS NULL)
			OR ("state" IN ('executing', 'applied', 'attachment_conflict', 'rejected', 'outcome_unknown') AND "decision_message_id" IS NOT NULL AND "approved" = 1 AND "decided_at" IS NOT NULL AND "execution_started_at" IS NOT NULL)
		),
	CONSTRAINT "hosted_project_clone_approvals_outcome_check" CHECK(
			("state" IN ('requested', 'approved', 'executing', 'denied') AND "project_json" IS NULL AND "attachment" IS NULL AND "rejection_reason" IS NULL AND "unknown_reason" IS NULL)
			OR ("state" = 'reused' AND "project_json" IS NOT NULL AND "attachment" IN ('attached', 'already_attached') AND "rejection_reason" IS NULL AND "unknown_reason" IS NULL)
			OR ("state" = 'applied' AND "project_json" IS NOT NULL AND "attachment" IN ('attached', 'already_attached') AND "rejection_reason" IS NULL AND "unknown_reason" IS NULL)
			OR ("state" = 'attachment_conflict' AND "project_json" IS NOT NULL AND "attachment" IS NULL AND "rejection_reason" IS NULL AND "unknown_reason" IS NULL)
			OR ("state" = 'rejected' AND "project_json" IS NULL AND "attachment" IS NULL AND "rejection_reason" IN ('destination_unavailable', 'provider_unavailable', 'repository_unavailable', 'thread_unavailable') AND "unknown_reason" IS NULL)
			OR ("state" = 'outcome_unknown' AND "project_json" IS NULL AND "attachment" IS NULL AND "rejection_reason" IS NULL AND "unknown_reason" IN ('interrupted', 'verification_failed'))
			),
	CONSTRAINT "hosted_project_clone_approvals_update_time_check" CHECK(
				("state" IN ('requested', 'reused') AND "updated_at" = "created_at")
				OR ("state" IN ('approved', 'denied') AND "updated_at" = "decided_at")
				OR ("state" = 'executing' AND "updated_at" = "execution_started_at")
				OR ("state" IN ('applied', 'attachment_conflict', 'rejected', 'outcome_unknown'))
			)
);
--> statement-breakpoint
CREATE TABLE `hosted_project_clone_artifacts` (
	`approval_id` text PRIMARY KEY,
	`request_json` text NOT NULL,
	`preparation_json` text NOT NULL,
	`destination_proof_json` text NOT NULL,
	`clone_result_json` text,
	`registered_project_json` text,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_hosted_project_clone_artifacts_approval_id_hosted_project_clone_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `hosted_project_clone_approvals`(`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_project_clone_artifacts_registration_pair_check" CHECK(("registered_project_json" IS NULL) OR ("clone_result_json" IS NOT NULL))
);
--> statement-breakpoint
CREATE TABLE `hosted_project_clone_claims` (
	`approval_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`canonical_root` text NOT NULL,
	`provider_id` text NOT NULL,
	`canonical_host` text NOT NULL,
	`native_id` text NOT NULL,
	`claim_token` text NOT NULL,
	`owner_instance_id` text DEFAULT 'unowned' NOT NULL,
	`claimed_at` text NOT NULL,
	`lease_expires_at` text NOT NULL,
	`execution_started_at` text,
	`execution_completed_at` text,
	CONSTRAINT `fk_hosted_project_clone_claims_approval_id_hosted_project_clone_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `hosted_project_clone_approvals`(`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "hosted_project_clone_claims_execution_pair_check" CHECK("execution_completed_at" IS NULL OR "execution_started_at" IS NOT NULL)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_project_clone_approvals_source_command_unique` ON `hosted_project_clone_approvals` (`source_command_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_project_clone_approvals_decision_message_unique` ON `hosted_project_clone_approvals` (`decision_message_id`);--> statement-breakpoint
CREATE INDEX `hosted_project_clone_approvals_thread_index` ON `hosted_project_clone_approvals` (`thread_id`);--> statement-breakpoint
CREATE INDEX `hosted_project_clone_approvals_state_index` ON `hosted_project_clone_approvals` (`state`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_project_clone_claims_destination_unique` ON `hosted_project_clone_claims` (`canonical_root`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_project_clone_claims_hosted_identity_unique` ON `hosted_project_clone_claims` (`provider_id`,`canonical_host`,`native_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `hosted_project_clone_claims_token_unique` ON `hosted_project_clone_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `hosted_project_clone_claims_thread_index` ON `hosted_project_clone_claims` (`thread_id`);--> statement-breakpoint
CREATE INDEX `hosted_project_clone_claims_lease_index` ON `hosted_project_clone_claims` (`lease_expires_at`);