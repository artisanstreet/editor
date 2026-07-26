CREATE TABLE `conversation_items` (
	`item_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`turn_id` text NOT NULL,
	`ordinal` integer NOT NULL,
	`entity_json` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `conversation_patches` (
	`patch_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`patch_json` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `conversation_sources` (
	`source_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`journal_sequence` integer,
	`observed_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `conversation_threads` (
	`thread_id` text PRIMARY KEY,
	`next_ordinal` integer DEFAULT 0 NOT NULL,
	`last_patch_sequence` integer DEFAULT 0 NOT NULL,
	`journal_sequence` integer DEFAULT 0 NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `conversation_turns` (
	`turn_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`ordinal` integer NOT NULL,
	`entity_json` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `conversation_items_thread_ordinal_unique` ON `conversation_items` (`thread_id`,`ordinal`);--> statement-breakpoint
CREATE INDEX `conversation_items_thread_turn_index` ON `conversation_items` (`thread_id`,`turn_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `conversation_patches_thread_sequence_unique` ON `conversation_patches` (`thread_id`,`sequence`);--> statement-breakpoint
CREATE INDEX `conversation_patches_thread_index` ON `conversation_patches` (`thread_id`,`sequence`);--> statement-breakpoint
CREATE UNIQUE INDEX `conversation_turns_thread_ordinal_unique` ON `conversation_turns` (`thread_id`,`ordinal`);--> statement-breakpoint
CREATE INDEX `conversation_turns_thread_index` ON `conversation_turns` (`thread_id`);
