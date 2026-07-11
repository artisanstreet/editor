CREATE TABLE `model_behaviour_settings` (
	`setting_id` text PRIMARY KEY,
	`value_json` text NOT NULL,
	`version` integer NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
INSERT INTO `model_behaviour_settings` (
	`setting_id`, `value_json`, `version`, `updated_at`
) VALUES (
	'auto_compaction_trigger_tokens', '{"type":"provider_default"}', 0, '1970-01-01T00:00:00.000Z'
);
--> statement-breakpoint
CREATE TABLE `model_behaviour_provider_states` (
	`provider_id` text NOT NULL,
	`setting_id` text NOT NULL,
	`status` text NOT NULL,
	`native_key` text,
	`target_path` text,
	`observed_hash` text,
	`applied_hash` text,
	`ignored_drift_hash` text,
	`backup_path` text,
	`last_error_code` text,
	`updated_at` text NOT NULL,
	PRIMARY KEY (`provider_id`, `setting_id`)
);
