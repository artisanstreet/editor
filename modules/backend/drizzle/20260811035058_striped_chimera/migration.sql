ALTER TABLE `orchestration_runs` ADD `last_observation_sequence` integer DEFAULT -1 NOT NULL;--> statement-breakpoint
UPDATE `orchestration_runs`
SET `last_observation_sequence` = COALESCE(
	(SELECT MAX(`sequence`)
	 FROM `orchestration_raw_observations`
	 WHERE `run_id` = `orchestration_runs`.`run_id`),
	-1
);--> statement-breakpoint

/** Raw provider frames were an unbounded duplicate of canonical projections. */
DROP TABLE `orchestration_raw_observations`;--> statement-breakpoint
CREATE TABLE `orchestration_raw_observations` (
	`observation_id` text PRIMARY KEY NOT NULL,
	`run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`native_id` text,
	`native_method` text,
	`transport` text NOT NULL,
	`protocol_version` text,
	`frame_json` text NOT NULL,
	`raw_frame_base64` text
);--> statement-breakpoint
CREATE INDEX `orchestration_raw_observations_run_sequence_index`
	ON `orchestration_raw_observations` (`run_id`,`sequence`);--> statement-breakpoint

/** Projection receipts are durable only for canonical journal facts. */
CREATE TABLE `conversation_sources_compacted` (
	`source_id` text PRIMARY KEY NOT NULL,
	`thread_id` text NOT NULL,
	`journal_sequence` integer,
	`observed_at` text NOT NULL
);--> statement-breakpoint
INSERT INTO `conversation_sources_compacted`
SELECT `source_id`, `thread_id`, `journal_sequence`, `observed_at`
FROM `conversation_sources`
WHERE `source_id` LIKE 'event:%';--> statement-breakpoint
DROP TABLE `conversation_sources`;--> statement-breakpoint
ALTER TABLE `conversation_sources_compacted` RENAME TO `conversation_sources`;--> statement-breakpoint

/** The surface is a bounded operational view; conversation tables own history. */
CREATE TABLE `surface_items_compacted` (
	`projection_order` integer PRIMARY KEY AUTOINCREMENT NOT NULL,
	`surface_id` text NOT NULL UNIQUE,
	`observation_id` text NOT NULL UNIQUE,
	`thread_id` text NOT NULL,
	`run_id` text NOT NULL,
	`group_id` text,
	`assignment_id` text,
	`sequence` integer NOT NULL,
	`category` text NOT NULL,
	`kind` text NOT NULL,
	`summary_json` text NOT NULL,
	`raw_origin_json` text,
	`occurred_at` text NOT NULL
);--> statement-breakpoint
INSERT INTO `surface_items_compacted`
SELECT `projection_order`, `surface_id`, `observation_id`, `thread_id`, `run_id`,
	`group_id`, `assignment_id`, `sequence`, `category`, `kind`, `summary_json`,
	`raw_origin_json`, `occurred_at`
FROM (
	SELECT *, ROW_NUMBER() OVER (
		PARTITION BY `thread_id` ORDER BY `projection_order` DESC
	) AS `retention_rank`
	FROM `surface_items`
)
WHERE `retention_rank` <= 512;--> statement-breakpoint
DROP TABLE `surface_items`;--> statement-breakpoint
ALTER TABLE `surface_items_compacted` RENAME TO `surface_items`;--> statement-breakpoint
CREATE INDEX `surface_items_thread_projection_order_index`
	ON `surface_items` (`thread_id`,`projection_order`);--> statement-breakpoint

/** Patches bridge live subscribers; snapshots own durable conversation state. */
CREATE TABLE `conversation_patches_compacted` (
	`patch_id` text PRIMARY KEY NOT NULL,
	`thread_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`patch_json` text NOT NULL
);--> statement-breakpoint
INSERT INTO `conversation_patches_compacted`
SELECT `patch_id`, `thread_id`, `sequence`, `patch_json`
FROM (
	SELECT `patch_id`, `thread_id`, `sequence`, `patch_json`,
		ROW_NUMBER() OVER (PARTITION BY `thread_id` ORDER BY `sequence` DESC) AS `retention_rank`
	FROM `conversation_patches`
)
WHERE `retention_rank` <= 256;--> statement-breakpoint
DROP TABLE `conversation_patches`;--> statement-breakpoint
ALTER TABLE `conversation_patches_compacted` RENAME TO `conversation_patches`;--> statement-breakpoint
CREATE UNIQUE INDEX `conversation_patches_thread_sequence_unique`
	ON `conversation_patches` (`thread_id`,`sequence`);--> statement-breakpoint

/** Inbox payloads are retained only while pending consumption. */
CREATE TABLE `native_subagent_transcript_inbox_compacted` (
	`observation_id` text PRIMARY KEY NOT NULL,
	`root_run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`agent_native_thread_id` text NOT NULL,
	`parent_native_thread_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`content_json` text NOT NULL,
	`created_at` text NOT NULL,
	`processed_at` text
);--> statement-breakpoint
INSERT INTO `native_subagent_transcript_inbox_compacted`
SELECT * FROM `native_subagent_transcript_inbox` WHERE `processed_at` IS NULL;--> statement-breakpoint
DROP TABLE `native_subagent_transcript_inbox`;--> statement-breakpoint
ALTER TABLE `native_subagent_transcript_inbox_compacted`
	RENAME TO `native_subagent_transcript_inbox`;--> statement-breakpoint
CREATE INDEX `native_subagent_transcript_inbox_pending_index`
	ON `native_subagent_transcript_inbox` (`processed_at`,`root_run_id`,`agent_native_thread_id`,`sequence`);--> statement-breakpoint

CREATE TABLE `native_subagent_observation_inbox_compacted` (
	`observation_id` text PRIMARY KEY NOT NULL,
	`root_run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`agent_native_thread_id` text NOT NULL,
	`parent_native_thread_id` text NOT NULL,
	`state` text NOT NULL,
	`sequence` integer NOT NULL,
	`activity` text,
	`agent_path` text,
	`turn_id` text,
	`native_id` text,
	`created_at` text NOT NULL,
	`processed_at` text
);--> statement-breakpoint
INSERT INTO `native_subagent_observation_inbox_compacted`
SELECT * FROM `native_subagent_observation_inbox` WHERE `processed_at` IS NULL;--> statement-breakpoint
DROP TABLE `native_subagent_observation_inbox`;--> statement-breakpoint
ALTER TABLE `native_subagent_observation_inbox_compacted`
	RENAME TO `native_subagent_observation_inbox`;--> statement-breakpoint
CREATE INDEX `native_subagent_observation_inbox_pending_index`
	ON `native_subagent_observation_inbox` (`processed_at`,`root_run_id`,`sequence`);--> statement-breakpoint

DROP INDEX IF EXISTS `conversation_patches_thread_index`;
