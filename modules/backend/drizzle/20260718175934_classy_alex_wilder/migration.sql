CREATE TABLE `orchestration_intake` (
	`message_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`engine_id` text NOT NULL,
	`working_directory` text NOT NULL,
	`text` text NOT NULL,
	`mentioned_projects_json` text,
	`raw_origin_json` text,
	`risk` text NOT NULL,
	`state` text NOT NULL,
	`question_id` text,
	`question` text,
	`assumptions_json` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `orchestration_intake_risk_check` CHECK (`risk` IN ('low', 'material', 'high', 'underspecified')),
	CONSTRAINT `orchestration_intake_state_check` CHECK (`state` IN ('pending', 'resolved')),
	CONSTRAINT `orchestration_intake_question_shape_check` CHECK ((`state` = 'pending' AND `question_id` IS NOT NULL AND `question` IS NOT NULL) OR (`state` = 'resolved'))
);
--> statement-breakpoint
CREATE UNIQUE INDEX `orchestration_intake_question_id_unique` ON `orchestration_intake` (`question_id`);
--> statement-breakpoint
CREATE INDEX `orchestration_intake_thread_state_index` ON `orchestration_intake` (`thread_id`,`state`);
--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `auto_steer_follow_ups` integer DEFAULT true NOT NULL;
