CREATE TABLE `export_control_audit_decisions` (
	`decision_id` text PRIMARY KEY,
	`action` text NOT NULL,
	`intent_fingerprint` text NOT NULL,
	`decision_json` text NOT NULL,
	`record_json` text NOT NULL,
	`created_at` text NOT NULL,
	CONSTRAINT "export_control_audit_action_check" CHECK("action" IN ('account', 'billing', 'distribution', 'hosted_sync', 'marketplace_delivery', 'release', 'update')),
	CONSTRAINT "export_control_audit_fingerprint_check" CHECK(length("intent_fingerprint") = 64 AND "intent_fingerprint" NOT GLOB '*[^0-9a-f]*'),
	CONSTRAINT "export_control_audit_decision_json_check" CHECK(json_valid("decision_json") = 1 AND json_type("decision_json") = 'object' AND length(CAST("decision_json" AS BLOB)) <= 8192),
	CONSTRAINT "export_control_audit_record_json_check" CHECK(json_valid("record_json") = 1 AND json_type("record_json") = 'object' AND length(CAST("record_json" AS BLOB)) <= 8192),
	CONSTRAINT "export_control_audit_created_at_check" CHECK(strftime('%Y-%m-%dT%H:%M:%fZ', "created_at") IS "created_at" AND substr("created_at", 12, 2) BETWEEN '00' AND '23')
);
--> statement-breakpoint
CREATE TABLE `surface_projection_generations` (
	`generation_id` text PRIMARY KEY,
	`watermark` integer NOT NULL,
	`stream_cursors_json` text NOT NULL,
	`item_count` integer NOT NULL,
	`created_at` text NOT NULL,
	CONSTRAINT "surface_projection_generations_watermark_check" CHECK("watermark" >= 0),
	CONSTRAINT "surface_projection_generations_item_count_check" CHECK("item_count" >= 0),
	CONSTRAINT "surface_projection_generations_cursors_check" CHECK(json_valid("stream_cursors_json") = 1 AND json_type("stream_cursors_json") = 'array'),
	CONSTRAINT "surface_projection_generations_created_at_check" CHECK(strftime('%Y-%m-%dT%H:%M:%fZ', "created_at") IS "created_at" AND substr("created_at", 12, 2) BETWEEN '00' AND '23')
);
--> statement-breakpoint
CREATE TABLE `surface_projection_items` (
	`generation_id` text NOT NULL,
	`surface_id` text NOT NULL,
	`item_json` text NOT NULL,
	CONSTRAINT `surface_projection_items_pk` PRIMARY KEY(`generation_id`, `surface_id`),
	CONSTRAINT `fk_surface_projection_items_generation_id_surface_projection_generations_generation_id_fk` FOREIGN KEY (`generation_id`) REFERENCES `surface_projection_generations`(`generation_id`) ON DELETE CASCADE,
	CONSTRAINT "surface_projection_items_json_check" CHECK(json_valid("item_json") = 1 AND json_type("item_json") = 'object' AND length(CAST("item_json" AS BLOB)) <= 32768)
);
--> statement-breakpoint
CREATE TABLE `surface_projection_state` (
	`state_id` integer PRIMARY KEY,
	`generation_id` text NOT NULL,
	CONSTRAINT `fk_surface_projection_state_generation_id_surface_projection_generations_generation_id_fk` FOREIGN KEY (`generation_id`) REFERENCES `surface_projection_generations`(`generation_id`) ON DELETE RESTRICT,
	CONSTRAINT "surface_projection_state_singleton_check" CHECK("state_id" = 1)
);
