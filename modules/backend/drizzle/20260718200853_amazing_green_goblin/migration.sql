CREATE TABLE `preview_dispatch_leases` (
	`lease_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`kind` text NOT NULL,
	`target_id` text,
	`session_id` text,
	`acquired_at` text NOT NULL,
	`expires_at` text NOT NULL,
	CONSTRAINT "preview_dispatch_leases_kind_check" CHECK("kind" IN ('launch', 'probe', 'inspection_open', 'inspection_health'))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `preview_dispatch_leases_thread_id_unique` ON `preview_dispatch_leases` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_dispatch_leases_expires_at_index` ON `preview_dispatch_leases` (`expires_at`);
--> statement-breakpoint
DROP TRIGGER `journal_events_reject_erasing_thread`;
--> statement-breakpoint
CREATE TRIGGER `journal_events_reject_erasing_thread`
BEFORE INSERT ON `journal_events`
WHEN EXISTS (
	SELECT 1 FROM `thread_tombstones` WHERE `thread_id` = NEW.`thread_id`
) OR (
	EXISTS (
		SELECT 1 FROM `thread_erasure_claims` WHERE `thread_id` = NEW.`thread_id`
	) AND NOT EXISTS (
		SELECT 1 FROM `preview_dispatch_leases`
		WHERE `thread_id` = NEW.`thread_id`
			AND `expires_at` > NEW.`occurred_at`
			AND `lease_id` = NEW.`correlation_id`
			AND `lease_id` = NEW.`causation_id`
			AND NEW.`origin` = 'backend'
			AND NEW.`stream_id` = 'thread:' || NEW.`thread_id`
			AND NEW.`raw_origin_json` IS NULL
			AND NEW.`run_id` IS NULL
			AND NEW.`agent_id` IS NULL
			AND (
				(`kind` IN ('launch', 'probe') AND NEW.`event_type` = 'preview.target.updated')
				OR (`kind` IN ('inspection_open', 'inspection_health') AND NEW.`event_type` IN ('preview.target.updated', 'preview.inspection.updated'))
			)
	) AND NOT (
		NEW.`event_type` = 'thread.erased'
		AND NEW.`payload_json` = '{"type":"thread.erased"}'
		AND NEW.`event_id` = 'thread_erased_' || NEW.`thread_id`
		AND NEW.`correlation_id` = NEW.`event_id`
		AND NEW.`causation_id` = NEW.`event_id`
		AND NEW.`origin` = 'backend'
		AND NEW.`stream_id` = 'thread:' || NEW.`thread_id`
		AND NEW.`raw_origin_json` IS NULL
		AND NEW.`run_id` IS NULL
		AND NEW.`agent_id` IS NULL
	)
)
BEGIN
	SELECT RAISE(ABORT, 'thread is being erased');
END;
