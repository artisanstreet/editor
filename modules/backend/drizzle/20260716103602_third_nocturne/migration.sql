PRAGMA foreign_keys=OFF;--> statement-breakpoint
CREATE TABLE `__new_orchestration_raw_observations` (
	`observation_id` text NOT NULL,
	`run_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`sequence` integer NOT NULL,
	`native_id` text,
	`native_method` text,
	`transport` text NOT NULL,
	`protocol_version` text,
	`frame_json` text NOT NULL,
	`raw_frame_base64` text,
	CONSTRAINT `orchestration_raw_observations_pk` PRIMARY KEY(`run_id`, `observation_id`)
);
--> statement-breakpoint
INSERT INTO `__new_orchestration_raw_observations`(`observation_id`, `run_id`, `engine_id`, `sequence`, `native_id`, `native_method`, `transport`, `protocol_version`, `frame_json`, `raw_frame_base64`) SELECT `observation_id`, `run_id`, `engine_id`, `sequence`, `native_id`, `native_method`, `transport`, `protocol_version`, `frame_json`, `raw_frame_base64` FROM `orchestration_raw_observations`;--> statement-breakpoint
DROP TABLE `orchestration_raw_observations`;--> statement-breakpoint
ALTER TABLE `__new_orchestration_raw_observations` RENAME TO `orchestration_raw_observations`;--> statement-breakpoint
PRAGMA foreign_keys=ON;--> statement-breakpoint
CREATE INDEX `orchestration_raw_observations_run_sequence_index` ON `orchestration_raw_observations` (`run_id`,`sequence`);