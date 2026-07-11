ALTER TABLE `threads` ADD `title_source` text NOT NULL DEFAULT 'initial';
--> statement-breakpoint
ALTER TABLE `threads` ADD `title_locked` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `live_status` text NOT NULL DEFAULT 'Idle';
--> statement-breakpoint
ALTER TABLE `threads` ADD `current_goal` text;
--> statement-breakpoint
ALTER TABLE `threads` ADD `rename_suggestion` text;
--> statement-breakpoint
ALTER TABLE `threads` ADD `last_activity_at` text NOT NULL DEFAULT '1970-01-01T00:00:00.000Z';
--> statement-breakpoint
ALTER TABLE `threads` ADD `activity_version` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `metadata_version` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `pinned` integer NOT NULL DEFAULT 0;
--> statement-breakpoint
ALTER TABLE `threads` ADD `archived_at` text;
--> statement-breakpoint
UPDATE `threads`
SET `current_goal` = `title`, `last_activity_at` = `updated_at`;
--> statement-breakpoint
CREATE INDEX `threads_retention_candidates_index` ON `threads` (`pinned`,`last_activity_at`);
--> statement-breakpoint
CREATE TABLE `thread_retention_policies` (
	`policy_id` integer PRIMARY KEY,
	`enabled` integer NOT NULL,
	`inactivity_days` integer NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
INSERT INTO `thread_retention_policies` (`policy_id`, `enabled`, `inactivity_days`, `updated_at`)
VALUES (1, 1, 7, '1970-01-01T00:00:00.000Z');
--> statement-breakpoint
CREATE TABLE `thread_erasure_claims` (
	`thread_id` text PRIMARY KEY,
	`claimed_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `thread_tombstones` (
	`thread_id` text PRIMARY KEY,
	`deleted_at` text NOT NULL
);
--> statement-breakpoint
CREATE TRIGGER `journal_commands_reject_erasing_thread`
BEFORE INSERT ON `journal_commands`
WHEN EXISTS (
	SELECT 1 FROM `thread_erasure_claims` WHERE `thread_id` = NEW.`thread_id`
) OR EXISTS (
	SELECT 1 FROM `thread_tombstones` WHERE `thread_id` = NEW.`thread_id`
)
BEGIN
	SELECT RAISE(ABORT, 'thread is being erased');
END;
--> statement-breakpoint
CREATE TRIGGER `journal_events_reject_erasing_thread`
BEFORE INSERT ON `journal_events`
WHEN EXISTS (
	SELECT 1 FROM `thread_tombstones` WHERE `thread_id` = NEW.`thread_id`
) OR (
	EXISTS (
		SELECT 1 FROM `thread_erasure_claims` WHERE `thread_id` = NEW.`thread_id`
	) AND NOT (
		NEW.`event_type` = 'thread.erased'
		AND NEW.`payload_json` = '{"type":"thread.erased"}'
		AND NEW.`event_id` = 'thread_erased_' || NEW.`thread_id`
		AND NEW.`correlation_id` = NEW.`event_id`
		AND NEW.`causation_id` = NEW.`event_id`
		AND NEW.`origin` = 'backend'
		AND NEW.`stream_id` = 'thread:' || NEW.`thread_id`
		AND NEW.`raw_origin_json` IS NULL
		AND NEW.`run_id` IS NULL
		AND NEW.`agent_id` IS NULL
	)
)
BEGIN
	SELECT RAISE(ABORT, 'thread is being erased');
END;
--> statement-breakpoint
CREATE TRIGGER `threads_reject_tombstone_reuse`
BEFORE INSERT ON `threads`
WHEN EXISTS (
	SELECT 1 FROM `thread_tombstones` WHERE `thread_id` = NEW.`thread_id`
)
BEGIN
	SELECT RAISE(ABORT, 'thread id was erased');
END;
