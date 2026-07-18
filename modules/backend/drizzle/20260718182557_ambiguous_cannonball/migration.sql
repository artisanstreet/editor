CREATE TABLE `projection_rebuild_locks` (
	`lock_id` integer PRIMARY KEY,
	`generation` integer NOT NULL
);
--> statement-breakpoint
INSERT INTO `projection_rebuild_locks` (`lock_id`, `generation`) VALUES (1, 0);
