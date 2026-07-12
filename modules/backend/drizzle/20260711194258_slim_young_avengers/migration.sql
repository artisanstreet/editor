CREATE TABLE `workspace_change_snapshots` (
	`change_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`state` text NOT NULL,
	`content` blob,
	`byte_count` integer,
	`content_hash` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "workspace_change_snapshots_state_check" CHECK("state" IN ('available', 'consumed')),
	CONSTRAINT "workspace_change_snapshots_content_check" CHECK(
				(
					"state" = 'available'
					AND "content" IS NOT NULL
					AND "byte_count" IS NOT NULL
					AND "content_hash" IS NOT NULL
					AND length("content") = "byte_count"
					AND "byte_count" BETWEEN 0 AND 4194304
					AND length("content_hash") = 64
					AND "content_hash" NOT GLOB '*[^0-9a-f]*'
				)
				OR (
					"state" = 'consumed'
					AND "content" IS NULL
					AND "byte_count" IS NULL
					AND "content_hash" IS NULL
				)
			)
);
--> statement-breakpoint
CREATE INDEX `workspace_change_snapshots_thread_id_index` ON `workspace_change_snapshots` (`thread_id`);