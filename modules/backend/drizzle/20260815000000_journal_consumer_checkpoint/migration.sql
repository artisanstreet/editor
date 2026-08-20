CREATE TABLE IF NOT EXISTS `journal_consumer_checkpoints` (
	`consumer_id` text PRIMARY KEY NOT NULL,
	`journal_sequence` integer NOT NULL CHECK (`journal_sequence` >= 0)
);
