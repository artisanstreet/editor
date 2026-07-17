CREATE TABLE `tool_thread_dispatch_state` (
	`thread_id` text PRIMARY KEY,
	`admission_version` integer DEFAULT 0 NOT NULL,
	`quiesced_at` text,
	CONSTRAINT `fk_tool_thread_dispatch_state_thread_id_threads_thread_id_fk` FOREIGN KEY (`thread_id`) REFERENCES `threads`(`thread_id`) ON DELETE CASCADE,
	CONSTRAINT "tool_thread_dispatch_state_admission_version_check" CHECK("admission_version" >= 0),
	CONSTRAINT "tool_thread_dispatch_state_quiesced_at_check" CHECK("quiesced_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "quiesced_at") IS "quiesced_at" AND substr("quiesced_at", 12, 2) BETWEEN '00' AND '23'))
);
