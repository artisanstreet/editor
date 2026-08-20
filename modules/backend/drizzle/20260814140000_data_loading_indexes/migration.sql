CREATE INDEX IF NOT EXISTS `journal_events_thread_sequence_index` ON `journal_events` (`thread_id`, `sequence`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `journal_events_thread_run_sequence_index` ON `journal_events` (`thread_id`, `run_id`, `sequence`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `journal_events_thread_type_sequence_index` ON `journal_events` (`thread_id`, `event_type`, `sequence`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `surface_usage_totals_assignment_id_index` ON `surface_usage_totals` (`assignment_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `surface_usage_totals_group_id_index` ON `surface_usage_totals` (`group_id`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `surface_items_thread_run_projection_order_index` ON `surface_items` (`thread_id`, `run_id`, `projection_order`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `surface_items_thread_kind_projection_order_index` ON `surface_items` (`thread_id`, `kind`, `projection_order`);--> statement-breakpoint
CREATE INDEX IF NOT EXISTS `surface_items_thread_group_projection_order_index` ON `surface_items` (`thread_id`, `group_id`, `projection_order`);
