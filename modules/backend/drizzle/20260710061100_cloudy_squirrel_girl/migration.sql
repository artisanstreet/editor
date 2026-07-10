CREATE TABLE `event_streams` (
	`stream_id` text PRIMARY KEY,
	`last_sequence` integer NOT NULL
);
--> statement-breakpoint
CREATE TABLE `journal_commands` (
	`message_id` text PRIMARY KEY,
	`schema_version` integer NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`causation_id` text,
	`origin` text NOT NULL,
	`raw_origin_json` text,
	`sent_at` text NOT NULL,
	`payload_type` text NOT NULL,
	`payload_json` text NOT NULL,
	`status` text NOT NULL,
	`accepted_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `journal_events` (
	`sequence` integer PRIMARY KEY AUTOINCREMENT,
	`stream_id` text NOT NULL,
	`stream_sequence` integer NOT NULL,
	`schema_version` integer NOT NULL,
	`event_id` text NOT NULL,
	`correlation_id` text NOT NULL,
	`causation_id` text NOT NULL,
	`origin` text NOT NULL,
	`raw_origin_json` text,
	`event_type` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text,
	`agent_id` text,
	`payload_json` text NOT NULL,
	`occurred_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `threads` (
	`thread_id` text PRIMARY KEY,
	`title` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `journal_events_event_id_unique` ON `journal_events` (`event_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `journal_events_stream_sequence_unique` ON `journal_events` (`stream_id`,`stream_sequence`);--> statement-breakpoint
CREATE INDEX `journal_events_correlation_id_index` ON `journal_events` (`correlation_id`);