ALTER TABLE `orchestration_coordinators` ADD `policy_model_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_profile_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_provider_route_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_variant_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_coordinators` ADD `policy_catalog_revision` text;
--> statement-breakpoint
ALTER TABLE `orchestration_runs` ADD `profile_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_runs` ADD `provider_route_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_runs` ADD `variant_id` text;
--> statement-breakpoint
ALTER TABLE `orchestration_runs` ADD `catalog_revision` text;
--> statement-breakpoint
ALTER TABLE `surface_usage_totals` ADD `cost_usd` real;
