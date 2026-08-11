ALTER TABLE `threads` ADD `reader_acknowledged_activity_at` text DEFAULT '1970-01-01T00:00:00.000Z' NOT NULL;
INSERT INTO `journal_events` (
	`stream_id`,
	`stream_sequence`,
	`schema_version`,
	`event_id`,
	`idempotency_key`,
	`correlation_id`,
	`causation_id`,
	`origin`,
	`raw_origin_json`,
	`event_type`,
	`thread_id`,
	`run_id`,
	`agent_id`,
	`payload_json`,
	`occurred_at`
)
SELECT
	`event_streams`.`stream_id`,
	`event_streams`.`last_sequence` + 1,
	1,
	'migration:20260810203620:attention:event:' || `threads`.`thread_id`,
	'migration:20260810203620:attention:' || `threads`.`thread_id`,
	'migration:20260810203620:attention:' || `threads`.`thread_id`,
	'migration:20260810203620:attention:' || `threads`.`thread_id`,
	'backend',
	NULL,
	'thread.attention.acknowledged',
	`threads`.`thread_id`,
	NULL,
	NULL,
	json_object(
		'reader_activity_at', `threads`.`reader_activity_at`,
		'type', 'thread.attention.acknowledged'
	),
	`threads`.`updated_at`
FROM `threads`
INNER JOIN `event_streams`
	ON `event_streams`.`stream_id` = 'thread:' || `threads`.`thread_id`
WHERE `threads`.`live_status` IN ('Failed to complete', 'Needs attention');
UPDATE `event_streams`
SET `last_sequence` = `last_sequence` + 1
WHERE `stream_id` IN (
	SELECT 'thread:' || `thread_id`
	FROM `threads`
	WHERE `live_status` IN ('Failed to complete', 'Needs attention')
);
UPDATE `threads`
SET `reader_acknowledged_activity_at` = `reader_activity_at`
WHERE `live_status` IN ('Failed to complete', 'Needs attention');
