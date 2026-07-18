CREATE TABLE `preview_commands` (
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
CREATE TABLE `preview_inspection_sessions` (
	`session_id` text PRIMARY KEY,
	`target_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`connector_id` text NOT NULL,
	`state` text NOT NULL,
	`reconnect_state` text NOT NULL,
	`last_error` text,
	`journal_sequence` integer NOT NULL,
	`opened_at` text NOT NULL,
	`closed_at` text,
	`updated_at` text NOT NULL,
	CONSTRAINT "preview_inspection_sessions_state_check" CHECK("state" IN ('open', 'closed', 'abandoned')),
	CONSTRAINT "preview_inspection_sessions_reconnect_state_check" CHECK("reconnect_state" IN ('connected', 'reconnecting', 'unavailable', 'error')),
	CONSTRAINT "preview_inspection_sessions_closed_check" CHECK(("state" = 'open' AND "closed_at" IS NULL) OR ("state" IN ('closed', 'abandoned') AND "closed_at" IS NOT NULL)),
	CONSTRAINT "preview_inspection_sessions_journal_sequence_check" CHECK("journal_sequence" > 0)
);
--> statement-breakpoint
CREATE TABLE `preview_targets` (
	`target_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`project_id` text NOT NULL,
	`url` text NOT NULL,
	`port` integer NOT NULL,
	`routes_json` text NOT NULL,
	`source_kind` text,
	`source_id` text,
	`state` text NOT NULL,
	`launch_state` text NOT NULL,
	`last_error` text,
	`health_json` text,
	`journal_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`removed_at` text,
	CONSTRAINT "preview_targets_state_check" CHECK("state" IN ('registered', 'healthy', 'unhealthy', 'stopped', 'removed')),
	CONSTRAINT "preview_targets_port_check" CHECK("port" BETWEEN 1 AND 65535),
	CONSTRAINT "preview_targets_launch_state_check" CHECK("launch_state" IN ('idle', 'launching', 'launched', 'unavailable', 'error')),
	CONSTRAINT "preview_targets_source_check" CHECK(("source_kind" IS NULL AND "source_id" IS NULL) OR ("source_kind" IN ('process', 'terminal') AND "source_id" IS NOT NULL)),
	CONSTRAINT "preview_targets_journal_sequence_check" CHECK("journal_sequence" > 0)
);
--> statement-breakpoint
CREATE INDEX `preview_commands_thread_id_index` ON `preview_commands` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_thread_id_index` ON `preview_inspection_sessions` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_inspection_sessions_target_id_index` ON `preview_inspection_sessions` (`target_id`);--> statement-breakpoint
CREATE INDEX `preview_targets_thread_id_index` ON `preview_targets` (`thread_id`);--> statement-breakpoint
CREATE INDEX `preview_targets_workspace_id_index` ON `preview_targets` (`workspace_id`);
