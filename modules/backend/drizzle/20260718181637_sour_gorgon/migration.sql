PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_preview_commands` (
	`message_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`action` text NOT NULL,
	`payload_json` text NOT NULL,
	`journal_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	CONSTRAINT "preview_commands_action_check" CHECK("action" IN ('register', 'probe', 'state', 'remove', 'launch', 'inspection_open', 'inspection_reconnect', 'inspection_close', 'recovery')),
	CONSTRAINT "preview_commands_journal_sequence_check" CHECK("journal_sequence" > 0)
);
--> statement-breakpoint
INSERT INTO `__new_preview_commands`(`message_id`, `thread_id`, `action`, `payload_json`, `journal_sequence`, `created_at`) SELECT `message_id`, `thread_id`, `action`, `payload_json`, `journal_sequence`, `created_at` FROM `preview_commands`;--> statement-breakpoint
DROP TABLE `preview_commands`;--> statement-breakpoint
ALTER TABLE `__new_preview_commands` RENAME TO `preview_commands`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `preview_commands_thread_id_index` ON `preview_commands` (`thread_id`);