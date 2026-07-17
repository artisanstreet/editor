CREATE TABLE `run_usage_samples` (
	`run_id` text NOT NULL,
	`scope_key` text NOT NULL,
	`sample_scope` text NOT NULL,
	`input_tokens` integer NOT NULL,
	`output_tokens` integer NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `run_usage_samples_pk` PRIMARY KEY(`run_id`, `scope_key`),
	CONSTRAINT "run_usage_samples_scope_check" CHECK("sample_scope" IN ('turn_total', 'run_total')),
	CONSTRAINT "run_usage_samples_input_tokens_check" CHECK("input_tokens" BETWEEN 0 AND 9007199254740991),
	CONSTRAINT "run_usage_samples_output_tokens_check" CHECK("output_tokens" BETWEEN 0 AND 9007199254740991)
);
--> statement-breakpoint
CREATE INDEX `run_usage_samples_run_scope_index` ON `run_usage_samples` (`run_id`,`sample_scope`);
