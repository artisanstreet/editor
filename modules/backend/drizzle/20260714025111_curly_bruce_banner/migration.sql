ALTER TABLE `workspace_git_mutation_claims` ADD `owner_instance_id` text DEFAULT 'legacy_expired' NOT NULL;--> statement-breakpoint
ALTER TABLE `workspace_git_mutation_claims` ADD `lease_expires_at` text DEFAULT '1970-01-01T00:00:00.000Z' NOT NULL;--> statement-breakpoint
ALTER TABLE `workspace_git_mutation_claims` ADD `execution_started_at` text;--> statement-breakpoint
ALTER TABLE `workspace_git_mutation_claims` ADD `execution_completed_at` text;--> statement-breakpoint
CREATE INDEX `workspace_git_mutation_claims_lease_index` ON `workspace_git_mutation_claims` (`lease_expires_at`);
--> statement-breakpoint
UPDATE `workspace_git_mutation_claims`
SET `execution_started_at` = `claimed_at`
WHERE `owner_instance_id` = 'legacy_expired'
	AND `execution_started_at` IS NULL;
--> statement-breakpoint
UPDATE `journal_commands`
SET `payload_json` = CASE
	WHEN json_valid(`payload_json`) THEN json_remove(`payload_json`, '$.request_fingerprint')
	ELSE `payload_json`
END
WHERE `schema_version` = 1
	AND `payload_type` = 'workspace.git.mutation.request';
