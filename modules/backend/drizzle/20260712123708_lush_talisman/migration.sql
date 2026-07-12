CREATE TABLE `workspace_mutation_payloads` (
	`message_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`state` text NOT NULL,
	`expected` blob,
	`expected_byte_count` integer,
	`expected_hash` text,
	`replacement` blob,
	`replacement_byte_count` integer,
	`replacement_hash` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT `fk_workspace_mutation_payloads_message_id_workspace_change_operations_message_id_fk` FOREIGN KEY (`message_id`) REFERENCES `workspace_change_operations`(`message_id`) ON DELETE CASCADE,
	CONSTRAINT "workspace_mutation_payloads_state_check" CHECK("state" IN ('available', 'consumed')),
	CONSTRAINT "workspace_mutation_payloads_content_check" CHECK(
				(
					"state" = 'available'
					AND "expected" IS NOT NULL
					AND "expected_byte_count" IS NOT NULL
					AND "expected_hash" IS NOT NULL
					AND length("expected") = "expected_byte_count"
					AND "expected_byte_count" BETWEEN 0 AND 4194304
					AND length("expected_hash") = 64
					AND "expected_hash" NOT GLOB '*[^0-9a-f]*'
					AND "replacement" IS NOT NULL
					AND "replacement_byte_count" IS NOT NULL
					AND "replacement_hash" IS NOT NULL
					AND length("replacement") = "replacement_byte_count"
					AND "replacement_byte_count" BETWEEN 0 AND 4194304
					AND length("replacement_hash") = 64
					AND "replacement_hash" NOT GLOB '*[^0-9a-f]*'
				)
				OR (
					"state" = 'consumed'
					AND "expected" IS NULL
					AND "expected_byte_count" IS NULL
					AND "expected_hash" IS NULL
					AND "replacement" IS NULL
					AND "replacement_byte_count" IS NULL
					AND "replacement_hash" IS NULL
				)
			)
);
--> statement-breakpoint
CREATE INDEX `workspace_mutation_payloads_thread_id_index` ON `workspace_mutation_payloads` (`thread_id`);