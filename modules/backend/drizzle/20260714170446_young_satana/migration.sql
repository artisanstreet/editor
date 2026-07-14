CREATE TABLE `external_wait_operations` (
	`operation_id` text PRIMARY KEY,
	`source_command_id` text NOT NULL,
	`kind` text NOT NULL,
	`wait_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`sent_at` text NOT NULL,
	`result_snapshot_json` text NOT NULL,
	`journal_sequence` integer NOT NULL,
	CONSTRAINT `fk_external_wait_operations_wait_id_external_waits_wait_id_fk` FOREIGN KEY (`wait_id`) REFERENCES `external_waits`(`wait_id`) ON DELETE CASCADE,
	CONSTRAINT "external_wait_operations_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "external_wait_operations_kind_check" CHECK("kind" IN ('request', 'cancel', 'manual_resume'))
);
--> statement-breakpoint
CREATE TABLE `external_wait_wake_outbox` (
	`outbox_id` text PRIMARY KEY,
	`wait_id` text NOT NULL,
	`trigger_fingerprint` text NOT NULL,
	`follow_up_command_id` text NOT NULL,
	`follow_up_run_id` text NOT NULL,
	`mode` text,
	`state` text NOT NULL,
	`lease_owner` text,
	`lease_expires_at` text,
	`trigger_json` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_external_wait_wake_outbox_wait_id_external_waits_wait_id_fk` FOREIGN KEY (`wait_id`) REFERENCES `external_waits`(`wait_id`) ON DELETE CASCADE,
	CONSTRAINT "external_wait_wake_outbox_fingerprint_check" CHECK(length("trigger_fingerprint") = 64 AND "trigger_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "external_wait_wake_outbox_state_check" CHECK("state" IN ('pending', 'claimed', 'settled', 'cancelled')),
	CONSTRAINT "external_wait_wake_outbox_mode_check" CHECK(
			(
				"state" = 'pending'
				AND "mode" IS NULL
				AND "lease_owner" IS NULL
				AND "lease_expires_at" IS NULL
			)
			OR (
				"state" = 'claimed'
				AND "mode" IS NULL
				AND "lease_owner" IS NOT NULL
				AND "lease_expires_at" IS NOT NULL
			)
			OR (
				"state" = 'settled'
				AND "mode" IN ('native_resume', 'linked_run')
				AND "lease_owner" IS NULL
				AND "lease_expires_at" IS NULL
			)
			OR (
				"state" = 'cancelled'
				AND "mode" IS NULL
				AND "lease_owner" IS NULL
				AND "lease_expires_at" IS NULL
			)
		)
);
--> statement-breakpoint
CREATE TABLE `external_waits` (
	`wait_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`project_id` text NOT NULL,
	`workspace_id` text NOT NULL,
	`source_run_id` text NOT NULL,
	`owner_json` text NOT NULL,
	`target_json` text NOT NULL,
	`gates_json` text NOT NULL,
	`baseline_json` text NOT NULL,
	`baseline_fingerprint` text NOT NULL,
	`request_fingerprint` text NOT NULL,
	`state` text NOT NULL,
	`state_json` text NOT NULL,
	`generation` integer DEFAULT 1 NOT NULL,
	`maximum_generation` integer DEFAULT 3 NOT NULL,
	`next_observation_at` text NOT NULL,
	`timeout_at` text NOT NULL,
	`observer_lease_owner` text,
	`observer_lease_expires_at` text,
	`source_closed_at` text,
	`version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_external_waits_project_id_projects_project_id_fk` FOREIGN KEY (`project_id`) REFERENCES `projects`(`project_id`) ON DELETE RESTRICT,
	CONSTRAINT "external_waits_fingerprint_check" CHECK(
			length("baseline_fingerprint") = 64
			AND "baseline_fingerprint" NOT GLOB '*[^0-9a-f]*'
			AND length("request_fingerprint") = 64
			AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'
		),
	CONSTRAINT "external_waits_generation_check" CHECK(
			"generation" >= 1
			AND "maximum_generation" >= "generation"
			AND "maximum_generation" <= 10
		),
	CONSTRAINT "external_waits_version_sequence_check" CHECK("version" >= 1 AND "journal_sequence" >= 1),
	CONSTRAINT "external_waits_state_check" CHECK("state" IN ('waiting', 'wake_pending', 'woken', 'suspended', 'cancelled', 'exhausted')),
	CONSTRAINT "external_waits_observer_lease_check" CHECK(
			(
				"observer_lease_owner" IS NULL
				AND "observer_lease_expires_at" IS NULL
			)
			OR (
				"observer_lease_owner" IS NOT NULL
				AND "observer_lease_expires_at" IS NOT NULL
			)
		)
);
--> statement-breakpoint
CREATE UNIQUE INDEX `external_wait_operations_source_command_unique` ON `external_wait_operations` (`source_command_id`);--> statement-breakpoint
CREATE INDEX `external_wait_operations_wait_index` ON `external_wait_operations` (`wait_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `external_wait_wake_outbox_wait_unique` ON `external_wait_wake_outbox` (`wait_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `external_wait_wake_outbox_command_unique` ON `external_wait_wake_outbox` (`follow_up_command_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `external_wait_wake_outbox_run_unique` ON `external_wait_wake_outbox` (`follow_up_run_id`);--> statement-breakpoint
CREATE INDEX `external_wait_wake_outbox_state_index` ON `external_wait_wake_outbox` (`state`,`lease_expires_at`);--> statement-breakpoint
CREATE UNIQUE INDEX `external_waits_source_run_unique` ON `external_waits` (`source_run_id`);--> statement-breakpoint
CREATE INDEX `external_waits_thread_index` ON `external_waits` (`thread_id`,`updated_at`,`wait_id`);--> statement-breakpoint
CREATE INDEX `external_waits_observation_index` ON `external_waits` (`state`,`next_observation_at`,`observer_lease_expires_at`);