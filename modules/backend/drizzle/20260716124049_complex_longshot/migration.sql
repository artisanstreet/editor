CREATE TABLE `tool_control_commands` (
	`command_id` text PRIMARY KEY,
	`kind` text NOT NULL,
	`invocation_id` text NOT NULL,
	`approval_id` text,
	`decision` text,
	`request_fingerprint` text NOT NULL,
	`accepted_at` text NOT NULL,
	CONSTRAINT `fk_tool_control_commands_invocation_id_tool_invocations_invocation_id_fk` FOREIGN KEY (`invocation_id`) REFERENCES `tool_invocations`(`invocation_id`) ON DELETE CASCADE,
	CONSTRAINT `tool_control_commands_invocation_approval_fk` FOREIGN KEY (`invocation_id`,`approval_id`) REFERENCES `tool_invocations`(`invocation_id`,`approval_id`) ON DELETE CASCADE,
	CONSTRAINT "tool_control_commands_kind_check" CHECK(
				("kind" = 'invoke' AND "approval_id" IS NULL AND "decision" IS NULL)
				OR ("kind" = 'decision' AND "approval_id" IS NOT NULL AND "decision" IN ('approved', 'denied'))
			),
	CONSTRAINT "tool_control_commands_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "tool_control_commands_accepted_at_check" CHECK(strftime('%Y-%m-%dT%H:%M:%fZ', "accepted_at") IS "accepted_at" AND substr("accepted_at", 12, 2) BETWEEN '00' AND '23')
);
--> statement-breakpoint
CREATE TABLE `tool_execution_claims` (
	`invocation_id` text PRIMARY KEY,
	`claim_token` text NOT NULL,
	`owner_instance_id` text NOT NULL,
	`claimed_at` text NOT NULL,
	`lease_expires_at` text NOT NULL,
	`launch_started_at` text,
	CONSTRAINT `fk_tool_execution_claims_invocation_id_tool_invocations_invocation_id_fk` FOREIGN KEY (`invocation_id`) REFERENCES `tool_invocations`(`invocation_id`) ON DELETE CASCADE,
	CONSTRAINT "tool_execution_claims_lease_time_check" CHECK("lease_expires_at" >= "claimed_at"),
	CONSTRAINT "tool_execution_claims_launch_time_check" CHECK("launch_started_at" IS NULL OR ("launch_started_at" >= "claimed_at" AND "launch_started_at" <= "lease_expires_at")),
	CONSTRAINT "tool_execution_claims_timestamp_format_check" CHECK(
				strftime('%Y-%m-%dT%H:%M:%fZ', "claimed_at") IS "claimed_at"
				AND substr("claimed_at", 12, 2) BETWEEN '00' AND '23'
				AND strftime('%Y-%m-%dT%H:%M:%fZ', "lease_expires_at") IS "lease_expires_at"
				AND substr("lease_expires_at", 12, 2) BETWEEN '00' AND '23'
				AND ("launch_started_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "launch_started_at") IS "launch_started_at" AND substr("launch_started_at", 12, 2) BETWEEN '00' AND '23'))
			)
);
--> statement-breakpoint
CREATE TABLE `tool_invocation_private` (
	`invocation_id` text PRIMARY KEY,
	`request_fingerprint` text NOT NULL,
	`arguments_json` text NOT NULL,
	`arguments_digest` text NOT NULL,
	`result_json` text,
	`result_digest` text,
	CONSTRAINT `fk_tool_invocation_private_invocation_id_tool_invocations_invocation_id_fk` FOREIGN KEY (`invocation_id`) REFERENCES `tool_invocations`(`invocation_id`) ON DELETE CASCADE,
	CONSTRAINT "tool_invocation_private_request_fingerprint_check" CHECK(length("request_fingerprint") = 64 AND "request_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "tool_invocation_private_arguments_digest_check" CHECK(length("arguments_digest") = 64 AND "arguments_digest" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "tool_invocation_private_arguments_size_check" CHECK(json_valid("arguments_json") = 1 AND length(CAST("arguments_json" AS BLOB)) <= 65536),
	CONSTRAINT "tool_invocation_private_result_size_check" CHECK("result_json" IS NULL OR (json_valid("result_json") = 1 AND length(CAST("result_json" AS BLOB)) <= 65536)),
	CONSTRAINT "tool_invocation_private_result_pair_check" CHECK(("result_json" IS NULL AND "result_digest" IS NULL) OR ("result_json" IS NOT NULL AND "result_digest" IS NOT NULL AND length("result_digest") = 64 AND "result_digest" NOT GLOB '*[^0-9a-f]*'))
);
--> statement-breakpoint
CREATE TABLE `tool_invocations` (
	`invocation_id` text PRIMARY KEY,
	`request_id` text NOT NULL,
	`thread_id` text NOT NULL,
	`run_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`workspace_id` text,
	`owner_kind` text NOT NULL,
	`tool_id` text NOT NULL,
	`revision` integer NOT NULL,
	`source` text NOT NULL,
	`effect` text NOT NULL,
	`approval_policy` text NOT NULL,
	`label` text NOT NULL,
	`summary` text NOT NULL,
	`input_schema_json` text NOT NULL,
	`descriptor_fingerprint` text NOT NULL,
	`recovery_policy` text NOT NULL,
	`approval_id` text,
	`decision_id` text,
	`decision` text,
	`state` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`decided_at` text,
	`started_at` text,
	`suspended_at` text,
	`settled_at` text,
	`current_journal_sequence` integer NOT NULL,
	CONSTRAINT "tool_invocations_owner_kind_check" CHECK("owner_kind" IN ('ordinary_run', 'graph_run')),
	CONSTRAINT "tool_invocations_revision_check" CHECK("revision" > 0),
	CONSTRAINT "tool_invocations_source_check" CHECK("source" IN ('artisan', 'marketplace')),
	CONSTRAINT "tool_invocations_effect_check" CHECK("effect" IN ('read', 'durable_state', 'workspace_mutation', 'unknown')),
	CONSTRAINT "tool_invocations_descriptor_fingerprint_check" CHECK(length("descriptor_fingerprint") = 64 AND "descriptor_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "tool_invocations_input_schema_size_check" CHECK(json_valid("input_schema_json") = 1 AND length(CAST("input_schema_json" AS BLOB)) <= 65536),
	CONSTRAINT "tool_invocations_recovery_policy_check" CHECK("recovery_policy" IN ('retry', 'outcome_unknown')),
	CONSTRAINT "tool_invocations_lifecycle_check" CHECK(
				(
					"approval_policy" = 'automatic'
					AND "approval_id" IS NULL
					AND "decision_id" IS NULL
					AND "decision" IS NULL
					AND "decided_at" IS NULL
					AND (
						("state" = 'pending' AND "started_at" IS NULL AND "suspended_at" IS NULL AND "settled_at" IS NULL)
						OR ("state" = 'running' AND "started_at" IS NOT NULL AND "suspended_at" IS NULL AND "settled_at" IS NULL)
						OR ("state" IN ('completed', 'failed', 'outcome_unknown') AND "started_at" IS NOT NULL AND "suspended_at" IS NULL AND "settled_at" IS NOT NULL)
						OR ("state" = 'suspended' AND "started_at" IS NOT NULL AND "suspended_at" IS NOT NULL AND "settled_at" IS NULL)
					)
				)
				OR (
					"approval_policy" = 'required'
					AND "approval_id" IS NOT NULL
					AND (
						("state" = 'approval_required' AND "decision_id" IS NULL AND "decision" IS NULL AND "decided_at" IS NULL AND "started_at" IS NULL AND "suspended_at" IS NULL AND "settled_at" IS NULL)
						OR ("state" = 'denied' AND "decision_id" IS NOT NULL AND "decision" = 'denied' AND "decided_at" IS NOT NULL AND "started_at" IS NULL AND "suspended_at" IS NULL AND "settled_at" IS NOT NULL)
						OR ("state" = 'pending' AND "decision_id" IS NOT NULL AND "decision" = 'approved' AND "decided_at" IS NOT NULL AND "started_at" IS NULL AND "suspended_at" IS NULL AND "settled_at" IS NULL)
						OR ("state" = 'running' AND "decision_id" IS NOT NULL AND "decision" = 'approved' AND "decided_at" IS NOT NULL AND "started_at" IS NOT NULL AND "suspended_at" IS NULL AND "settled_at" IS NULL)
						OR ("state" IN ('completed', 'failed', 'outcome_unknown') AND "decision_id" IS NOT NULL AND "decision" = 'approved' AND "decided_at" IS NOT NULL AND "started_at" IS NOT NULL AND "suspended_at" IS NULL AND "settled_at" IS NOT NULL)
						OR ("state" = 'suspended' AND "decision_id" IS NOT NULL AND "decision" = 'approved' AND "decided_at" IS NOT NULL AND "started_at" IS NOT NULL AND "suspended_at" IS NOT NULL AND "settled_at" IS NULL)
					)
				)
			),
	CONSTRAINT "tool_invocations_timestamp_format_check" CHECK(
				strftime('%Y-%m-%dT%H:%M:%fZ', "created_at") IS "created_at"
				AND substr("created_at", 12, 2) BETWEEN '00' AND '23'
				AND strftime('%Y-%m-%dT%H:%M:%fZ', "updated_at") IS "updated_at"
				AND substr("updated_at", 12, 2) BETWEEN '00' AND '23'
				AND ("decided_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "decided_at") IS "decided_at" AND substr("decided_at", 12, 2) BETWEEN '00' AND '23'))
				AND ("started_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "started_at") IS "started_at" AND substr("started_at", 12, 2) BETWEEN '00' AND '23'))
				AND ("suspended_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "suspended_at") IS "suspended_at" AND substr("suspended_at", 12, 2) BETWEEN '00' AND '23'))
				AND ("settled_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "settled_at") IS "settled_at" AND substr("settled_at", 12, 2) BETWEEN '00' AND '23'))
			),
	CONSTRAINT "tool_invocations_timestamp_order_check" CHECK(
				"created_at" <= "updated_at"
				AND ("decided_at" IS NULL OR ("decided_at" >= "created_at" AND "decided_at" <= "updated_at"))
				AND ("started_at" IS NULL OR ("started_at" >= "created_at" AND "started_at" <= "updated_at"))
				AND ("suspended_at" IS NULL OR ("suspended_at" >= "created_at" AND "suspended_at" <= "updated_at"))
				AND ("settled_at" IS NULL OR ("settled_at" >= "created_at" AND "settled_at" <= "updated_at"))
				AND ("decided_at" IS NULL OR "started_at" IS NULL OR "decided_at" <= "started_at")
				AND ("started_at" IS NULL OR "suspended_at" IS NULL OR "started_at" <= "suspended_at")
				AND ("started_at" IS NULL OR "settled_at" IS NULL OR "started_at" <= "settled_at")
				AND ("decided_at" IS NULL OR "settled_at" IS NULL OR "decided_at" <= "settled_at")
			),
	CONSTRAINT "tool_invocations_journal_sequence_check" CHECK("current_journal_sequence" > 0)
);
--> statement-breakpoint
CREATE INDEX `tool_control_commands_invocation_index` ON `tool_control_commands` (`invocation_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `tool_execution_claims_token_unique` ON `tool_execution_claims` (`claim_token`);--> statement-breakpoint
CREATE INDEX `tool_execution_claims_lease_index` ON `tool_execution_claims` (`lease_expires_at`);--> statement-breakpoint
CREATE UNIQUE INDEX `tool_invocations_request_unique` ON `tool_invocations` (`request_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `tool_invocations_approval_unique` ON `tool_invocations` (`approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `tool_invocations_decision_unique` ON `tool_invocations` (`decision_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `tool_invocations_invocation_approval_unique` ON `tool_invocations` (`invocation_id`,`approval_id`);--> statement-breakpoint
CREATE INDEX `tool_invocations_thread_index` ON `tool_invocations` (`thread_id`);