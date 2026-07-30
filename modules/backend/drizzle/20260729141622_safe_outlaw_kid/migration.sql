CREATE TABLE `session_defaults` (
	`defaults_id` integer PRIMARY KEY,
	`last_model_id` text,
	`permission` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `session_model_defaults` (
	`context_window` text,
	`model_id` text PRIMARY KEY,
	`reasoning_effort` text,
	`updated_at` text NOT NULL
);
