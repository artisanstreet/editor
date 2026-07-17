CREATE TABLE `routine_install_approvals` (
	`approval_id` text PRIMARY KEY,
	`preview_operation_id` text NOT NULL,
	`preview_json` text NOT NULL,
	`preview_fingerprint` text NOT NULL,
	`decision` text DEFAULT 'pending' NOT NULL,
	`decision_id` text,
	`decision_operation_id` text,
	`decision_request_json` text,
	`decision_snapshot_json` text,
	`decided_at` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	CONSTRAINT "routine_install_approvals_decision_check" CHECK("decision" IN ('pending', 'approved', 'denied', 'applied')),
	CONSTRAINT "routine_install_approvals_decision_pair_check" CHECK(("decision" = 'pending' AND "decision_id" IS NULL AND "decision_operation_id" IS NULL AND "decision_request_json" IS NULL AND "decision_snapshot_json" IS NULL AND "decided_at" IS NULL) OR ("decision" IN ('approved', 'denied', 'applied') AND "decision_id" IS NOT NULL AND "decision_operation_id" IS NOT NULL AND "decision_request_json" IS NOT NULL AND "decision_snapshot_json" IS NOT NULL AND "decided_at" IS NOT NULL)),
	CONSTRAINT "routine_install_approvals_preview_fingerprint_check" CHECK(length("preview_fingerprint") = 64 AND "preview_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "routine_install_approvals_preview_json_check" CHECK(json_valid("preview_json") = 1 AND json_type("preview_json") = 'object' AND length(CAST("preview_json" AS BLOB)) <= 8388608),
	CONSTRAINT "routine_install_approvals_decision_json_check" CHECK(("decision_request_json" IS NULL AND "decision_snapshot_json" IS NULL) OR (json_valid("decision_request_json") = 1 AND json_type("decision_request_json") = 'object' AND length(CAST("decision_request_json" AS BLOB)) <= 8388608 AND json_valid("decision_snapshot_json") = 1 AND json_type("decision_snapshot_json") = 'object' AND length(CAST("decision_snapshot_json" AS BLOB)) <= 8388608)),
	CONSTRAINT "routine_install_approvals_timestamp_check" CHECK(strftime('%Y-%m-%dT%H:%M:%fZ', "created_at") IS "created_at" AND substr("created_at", 12, 2) BETWEEN '00' AND '23' AND strftime('%Y-%m-%dT%H:%M:%fZ', "updated_at") IS "updated_at" AND substr("updated_at", 12, 2) BETWEEN '00' AND '23' AND ("decided_at" IS NULL OR (strftime('%Y-%m-%dT%H:%M:%fZ', "decided_at") IS "decided_at" AND substr("decided_at", 12, 2) BETWEEN '00' AND '23'))),
	CONSTRAINT "routine_install_approvals_timestamp_order_check" CHECK("created_at" <= "updated_at" AND ("decided_at" IS NULL OR ("decided_at" >= "created_at" AND "decided_at" <= "updated_at")) AND ("decision" IN ('pending', 'applied') OR "decided_at" = "updated_at"))
);
--> statement-breakpoint
CREATE TABLE `routine_installation_history` (
	`installation_id` text PRIMARY KEY,
	`approval_id` text NOT NULL,
	`install_operation_id` text NOT NULL,
	`routine_id` text NOT NULL,
	`scope_slot` text NOT NULL,
	`install_version` integer NOT NULL,
	`routine_json` text NOT NULL,
	`rollback_identity_json` text NOT NULL,
	`rollback_id` text NOT NULL,
	`rollback_plan_fingerprint` text NOT NULL,
	`rollback_plan_version` integer NOT NULL,
	`is_active` integer DEFAULT true NOT NULL,
	`installed_at` text NOT NULL,
	CONSTRAINT `fk_routine_installation_history_approval_id_routine_install_approvals_approval_id_fk` FOREIGN KEY (`approval_id`) REFERENCES `routine_install_approvals`(`approval_id`) ON DELETE RESTRICT,
	CONSTRAINT "routine_installation_history_version_check" CHECK("install_version" > 0),
	CONSTRAINT "routine_installation_history_active_check" CHECK("is_active" IN (0, 1)),
	CONSTRAINT "routine_installation_history_routine_json_check" CHECK(json_valid("routine_json") = 1 AND json_type("routine_json") = 'object' AND length(CAST("routine_json" AS BLOB)) <= 8388608),
	CONSTRAINT "routine_installation_history_rollback_identity_json_check" CHECK(json_valid("rollback_identity_json") = 1 AND json_type("rollback_identity_json") = 'object' AND length(CAST("rollback_identity_json" AS BLOB)) <= 16384),
	CONSTRAINT "routine_installation_history_rollback_fingerprint_check" CHECK(length("rollback_plan_fingerprint") = 64 AND "rollback_plan_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "routine_installation_history_timestamp_check" CHECK(strftime('%Y-%m-%dT%H:%M:%fZ', "installed_at") IS "installed_at" AND substr("installed_at", 12, 2) BETWEEN '00' AND '23')
);
--> statement-breakpoint
CREATE UNIQUE INDEX `routine_install_approvals_preview_operation_unique` ON `routine_install_approvals` (`preview_operation_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `routine_install_approvals_decision_unique` ON `routine_install_approvals` (`decision_id`) WHERE "routine_install_approvals"."decision_id" IS NOT NULL;--> statement-breakpoint
CREATE UNIQUE INDEX `routine_install_approvals_decision_operation_unique` ON `routine_install_approvals` (`decision_operation_id`) WHERE "routine_install_approvals"."decision_operation_id" IS NOT NULL;--> statement-breakpoint
CREATE UNIQUE INDEX `routine_installation_history_approval_unique` ON `routine_installation_history` (`approval_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `routine_installation_history_operation_unique` ON `routine_installation_history` (`install_operation_id`);--> statement-breakpoint
CREATE UNIQUE INDEX `routine_installation_history_slot_version_unique` ON `routine_installation_history` (`routine_id`,`scope_slot`,`install_version`);--> statement-breakpoint
CREATE UNIQUE INDEX `routine_installation_history_active_slot_unique` ON `routine_installation_history` (`routine_id`,`scope_slot`) WHERE "routine_installation_history"."is_active" = 1;