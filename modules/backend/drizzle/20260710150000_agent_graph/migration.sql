CREATE TABLE `orchestration_groups` (
	`group_id` text PRIMARY KEY,
	`thread_id` text NOT NULL,
	`coordinator_agent_id` text NOT NULL,
	`state` text NOT NULL,
	`max_concurrency` integer NOT NULL,
	`version` integer NOT NULL,
	`journal_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_groups_thread_id_index` ON `orchestration_groups` (`thread_id`);
--> statement-breakpoint
CREATE INDEX `orchestration_groups_state_index` ON `orchestration_groups` (`state`);
--> statement-breakpoint
CREATE TABLE `agent_instances` (
	`agent_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`display_name` text NOT NULL,
	`role` text NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE UNIQUE INDEX `agent_instances_group_display_name_unique` ON `agent_instances` (`group_id`,`display_name`);
--> statement-breakpoint
CREATE INDEX `agent_instances_group_id_index` ON `agent_instances` (`group_id`);
--> statement-breakpoint
CREATE TABLE `assignments` (
	`assignment_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`role` text NOT NULL,
	`scope_json` text NOT NULL,
	`engine_id` text NOT NULL,
	`profile` text NOT NULL,
	`workspace_json` text NOT NULL,
	`permission_policy_json` text NOT NULL,
	`summary_contract` text NOT NULL,
	`parent_node_id` text NOT NULL,
	`expected_result` text NOT NULL,
	`instructions` text NOT NULL,
	`state` text NOT NULL,
	`current_attempt` integer NOT NULL,
	`max_attempts` integer NOT NULL,
	`active_run_id` text,
	`heartbeat_json` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `assignments_group_id_index` ON `assignments` (`group_id`);
--> statement-breakpoint
CREATE INDEX `assignments_state_index` ON `assignments` (`state`);
--> statement-breakpoint
CREATE INDEX `assignments_active_run_id_index` ON `assignments` (`active_run_id`);
--> statement-breakpoint
CREATE TABLE `agent_runs` (
	`run_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`assignment_id` text NOT NULL,
	`agent_id` text NOT NULL,
	`attempt` integer NOT NULL,
	`engine_id` text NOT NULL,
	`profile` text NOT NULL,
	`state` text NOT NULL,
	`dispatch_status` text NOT NULL,
	`owner_instance_id` text,
	`native_thread_id` text,
	`native_resume_json` text,
	`native_identity_json` text,
	`raw_origin_json` text,
	`last_observation_sequence` integer NOT NULL,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL,
	`completed_at` text
);
--> statement-breakpoint
CREATE UNIQUE INDEX `agent_runs_assignment_attempt_unique` ON `agent_runs` (`assignment_id`,`attempt`);
--> statement-breakpoint
CREATE INDEX `agent_runs_group_id_index` ON `agent_runs` (`group_id`);
--> statement-breakpoint
CREATE INDEX `agent_runs_dispatch_status_index` ON `agent_runs` (`dispatch_status`);
--> statement-breakpoint
CREATE INDEX `agent_runs_assignment_id_index` ON `agent_runs` (`assignment_id`);
--> statement-breakpoint
CREATE TABLE `orchestration_joins` (
	`join_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`strategy` text NOT NULL,
	`state` text NOT NULL,
	`upstream_assignment_ids_json` text NOT NULL,
	`downstream_assignment_id` text,
	`selected_assignment_id` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_joins_group_id_index` ON `orchestration_joins` (`group_id`);
--> statement-breakpoint
CREATE TABLE `orchestration_graph_edges` (
	`edge_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`from_node_id` text NOT NULL,
	`to_node_id` text NOT NULL,
	`kind` text NOT NULL,
	`dispatch_dependency` integer NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_graph_edges_group_id_index` ON `orchestration_graph_edges` (`group_id`);
--> statement-breakpoint
CREATE TABLE `orchestration_artifacts` (
	`artifact_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`assignment_id` text NOT NULL,
	`run_id` text NOT NULL,
	`kind` text NOT NULL,
	`label` text NOT NULL,
	`content` text,
	`uri` text,
	`raw_origin_json` text,
	`created_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_artifacts_group_id_index` ON `orchestration_artifacts` (`group_id`);
--> statement-breakpoint
CREATE INDEX `orchestration_artifacts_assignment_id_index` ON `orchestration_artifacts` (`assignment_id`);
--> statement-breakpoint
CREATE TABLE `orchestration_graph_commands` (
	`message_id` text PRIMARY KEY,
	`group_id` text NOT NULL,
	`assignment_id` text,
	`run_id` text,
	`action` text NOT NULL,
	`status` text NOT NULL,
	`outcome` text,
	`journal_sequence` integer,
	`failure` text,
	`created_at` text NOT NULL,
	`updated_at` text NOT NULL
);
--> statement-breakpoint
CREATE INDEX `orchestration_graph_commands_group_id_index` ON `orchestration_graph_commands` (`group_id`);
--> statement-breakpoint
CREATE INDEX `orchestration_graph_commands_status_index` ON `orchestration_graph_commands` (`status`);
