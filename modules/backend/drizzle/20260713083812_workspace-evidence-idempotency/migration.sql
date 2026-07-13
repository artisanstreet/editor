ALTER TABLE `journal_events` ADD `idempotency_key` text;--> statement-breakpoint
CREATE UNIQUE INDEX `journal_events_idempotency_key_unique` ON `journal_events` (`idempotency_key`);