CREATE TABLE `surface_items` (
	`projection_order` integer PRIMARY KEY AUTOINCREMENT,
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
);
--> statement-breakpoint
CREATE TABLE `surface_usage_totals` (
	`run_id` text PRIMARY KEY,
	`group_id` text,
	`assignment_id` text,
	`input_tokens` integer,
	`output_tokens` integer,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `surface_items_thread_projection_order_index` ON `surface_items` (`thread_id`,`projection_order`);
