ALTER TABLE `threads` ADD `summary_title` text;--> statement-breakpoint
ALTER TABLE `session_defaults` ADD `thread_title_mode` text DEFAULT 'summary' NOT NULL;
