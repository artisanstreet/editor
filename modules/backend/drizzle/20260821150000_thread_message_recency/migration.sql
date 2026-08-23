ALTER TABLE `threads` ADD `last_message_at` text NOT NULL DEFAULT '1970-01-01T00:00:00.000Z';
--> statement-breakpoint
UPDATE `threads`
SET `last_message_at` = COALESCE(
	(
		SELECT MAX(`journal_events`.`occurred_at`)
		FROM `journal_events`
		WHERE `journal_events`.`thread_id` = `threads`.`thread_id`
			AND `journal_events`.`event_type` IN (
				'thread.message_queued',
				'thread.message_steering',
				'assistant.message_completed'
			)
	),
	`threads`.`created_at`
);
