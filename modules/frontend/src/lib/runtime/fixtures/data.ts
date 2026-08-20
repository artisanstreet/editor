import type {
	EventEnvelope,
	GlobalGuidanceSnapshot,
	GitWorkspaceQueryResult,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	SurfaceUsageDailyBucket,
	PreviewTarget,
	RichLinkAssetMetadata,
	RichLinkResolution,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	ThreadTranscriptSnapshot,
	WorkspaceChange,
	WorkspaceChangeDiffQueryResult,
	WorkspaceFileReadQueryResult,
} from "@artisan/protocol";
import type { ArtisanClientCursors } from "@artisan/transport/client";

export { FixtureConversation } from "./conversation";

/**
 * Holds deterministic renderer data for visual fixtures and contract tests.
 * This module is never a production composition root.
 */
export interface FixtureArtisanClientData {
	readonly asset_output: Readonly<Record<string, Uint8Array>>;
	readonly cursors: ArtisanClientCursors;
	readonly events: ReadonlyArray<EventEnvelope>;
	readonly global_guidance: GlobalGuidanceSnapshot;
	readonly git_workspace: GitWorkspaceQueryResult;
	readonly model_behaviour: ModelBehaviourSnapshot;
	readonly orchestration_graph: OrchestrationGraph;
	readonly orchestration_groups: OrchestrationGroupListSnapshot;
	readonly surface_usage_daily: ReadonlyArray<SurfaceUsageDailyBucket>;
	readonly transcript: ThreadTranscriptSnapshot;
	readonly preview_assets: Readonly<Record<string, RichLinkAssetMetadata>>;
	readonly preview_targets: ReadonlyArray<PreviewTarget>;
	readonly rich_links: Readonly<Record<string, RichLinkResolution>>;
	readonly terminal_output: Readonly<Record<string, Uint8Array>>;
	readonly terminals: ReadonlyArray<TerminalSession>;
	readonly thread_retention_policy: ThreadRetentionPolicy;
	readonly thread_work: ThreadWorkItem;
	readonly threads: ReadonlyArray<ThreadListItem>;
	readonly workspace_changes: ReadonlyArray<WorkspaceChange>;
	readonly workspace_change_diffs: Readonly<Record<string, WorkspaceChangeDiffQueryResult>>;
	readonly workspace_files: Readonly<Record<string, WorkspaceFileReadQueryResult>>;
}

export const fixture_timestamp = "2026-07-12T10:00:00.000Z";

/** Claude's session usage window resets a few hours after the fixture's base timestamp. */
export const fixture_engine_usage_session_reset_at = new Date(
	Date.parse(fixture_timestamp) + 3 * 60 * 60 * 1000,
).toISOString();

/** The fixture project's HEAD was committed a few hours before the base timestamp. */
export const fixture_project_head_committed_at = new Date(
	Date.parse(fixture_timestamp) - 5 * 60 * 60 * 1000,
).toISOString();

export const fixture_project = {
	attached_at: fixture_timestamp,
	display_name: "Artisan Editor",
	project_id: "project-artisan-editor",
	root_path: "C:\\Users\\Sander\\Desktop\\artisan-editor",
	updated_at: fixture_timestamp,
} as const;

export const fixture_artisan_client_data = {
	asset_output: {
		"asset-readme": new Uint8Array([35, 32, 65, 114, 116, 105, 115, 97, 110, 10]),
	} as Readonly<Record<string, Uint8Array>>,
	cursors: {
		event_cursors: [{ sequence: 12, stream_id: "fixture-events" }],
		last_journal_sequence: 48,
	},
	events: [],
	git_workspace: {
		journal_sequence: 48,
		pending_mutations: [],
		workspace: {
			aggregate: {
				binary_file_count: 0,
				lines_added: 1,
				lines_deleted: 1,
				tracked_file_count: 1,
			},
			branch: { name: "main", type: "attached" },
			clean: false,
			files: [
				{
					flags: {
						conflicted: false,
						staged: false,
						unstaged: true,
						untracked: false,
					},
					path: "modules/frontend/src/lib/fixture.ts",
					porcelain_status: ".M",
				},
			],
			head: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
			journal_sequence: 48,
			observed_at: fixture_timestamp,
			repository_state: "repository",
			snapshot_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
			staged: {
				binary_file_count: 0,
				lines_added: 0,
				lines_deleted: 0,
				tracked_file_count: 0,
			},
			unstaged: {
				binary_file_count: 0,
				lines_added: 1,
				lines_deleted: 1,
				tracked_file_count: 1,
			},
			version: 1,
			workspace_id: "workspace-artisan-editor",
			worktrees: [
				{
					bare: false,
					branch: { name: "main", type: "attached" },
					head: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
					is_current: true,
					locked: false,
					path: "C:/Users/Sander/Desktop/artisan-editor",
					prunable: false,
					worktree_id: "worktree-artisan-editor",
				},
			],
		},
	},
	global_guidance: {
		candidates: [],
		content: "Build calm, legible tools with explicit ownership.\n",
		metadata: {
			canonical: {
				byte_count: 51,
				content_hash: "ca0dc114882269301bc660b649b91aa333db9ffe44484086bceb5c239670e95e",
				selected_provider: "codex",
				status: "ready",
				updated_at: fixture_timestamp,
			},
			providers: [
				{
					applied_byte_count: 51,
					applied_hash:
						"ca0dc114882269301bc660b649b91aa333db9ffe44484086bceb5c239670e95e",
					provider: "codex",
					status: "synced",
					updated_at: fixture_timestamp,
				},
				{
					provider: "claude",
					status: "applied_at_run_time",
					updated_at: fixture_timestamp,
				},
			],
		},
	},
	model_behaviour: {
		capabilities: [
			{
				control: {
					kind: "integer",
					maximum: 2_000_000,
					minimum: 16_384,
					step: 128,
					unit: "tokens",
				},
				description: "Compacts a session before its provider context is exhausted.",
				display_name: "Auto-compaction trigger",
				provider_support: [
					{
						activation_timing: "next_turn",
						details: "Applied through the curated Codex mapping.",
						native_key: "model_auto_compact_token_limit",
						provider_id: "codex",
						state: "supported",
					},
				],
				scope: "global_default",
				setting_id: "auto_compaction_trigger_tokens",
			},
		],
		providers: [
			{
				native_key: "model_auto_compact_token_limit",
				provider_id: "codex",
				setting_id: "auto_compaction_trigger_tokens",
				status: "synced",
				updated_at: fixture_timestamp,
			},
		],
		registry_version: 1,
		settings: [
			{
				setting_id: "auto_compaction_trigger_tokens",
				updated_at: fixture_timestamp,
				value: { type: "integer", value: 120_000 },
				version: 3,
			},
		],
	},
	orchestration_graph: {
		agent_instances: [
			{
				agent_id: "agent-terra",
				created_at: fixture_timestamp,
				display_name: "Terra",
				group_id: "group-editor-shell",
				role: "Frontend implementation",
				updated_at: fixture_timestamp,
			},
		],
		agent_runs: [],
		artifacts: [],
		assignments: [],
		edges: [],
		group: {
			coordinator_agent_id: "agent-sol",
			created_at: fixture_timestamp,
			group_id: "group-editor-shell",
			max_concurrency: 4,
			state: "running",
			thread_id: "thread-editor-shell",
			updated_at: fixture_timestamp,
			version: 2,
		},
		joins: [],
		journal_sequence: 48,
	},
	orchestration_groups: {
		groups: [
			{
				coordinator_agent_id: "agent-sol",
				created_at: fixture_timestamp,
				group_id: "group-editor-shell",
				max_concurrency: 4,
				state: "running",
				thread_id: "thread-editor-shell",
				updated_at: fixture_timestamp,
				version: 2,
			},
			{
				coordinator_agent_id: "agent-sol",
				created_at: fixture_timestamp,
				group_id: "group-editor-shell-complete",
				max_concurrency: 2,
				state: "complete",
				thread_id: "thread-editor-shell",
				updated_at: fixture_timestamp,
				version: 1,
			},
		],
		journal_sequence: 48,
	},
	surface_usage_daily: [
		{
			date: "2026-07-24",
			engines: [
				{
					engine_id: "claude",
					input_tokens: 35_910,
					model_id: "claude-fable-5",
					output_tokens: 4_950,
				},
				{
					engine_id: "codex",
					input_tokens: 12_210,
					model_id: "gpt-5.6-sol",
					output_tokens: 1_480,
				},
			],
			input_tokens: 48_120,
			output_tokens: 6_430,
		},
		{
			date: "2026-07-25",
			engines: [
				{
					engine_id: "claude",
					input_tokens: 88_140,
					model_id: "claude-opus-5",
					output_tokens: 12_610,
				},
				{
					engine_id: "codex",
					input_tokens: 39_530,
					model_id: "gpt-5.3-codex-spark",
					output_tokens: 5_240,
				},
				{ input_tokens: 4_220, output_tokens: 390 },
			],
			input_tokens: 131_890,
			output_tokens: 18_240,
		},
		{
			date: "2026-07-26",
			engines: [
				{
					engine_id: "codex",
					input_tokens: 22_470,
					model_id: "gpt-5.6-terra",
					output_tokens: 3_110,
				},
			],
			input_tokens: 22_470,
			output_tokens: 3_110,
		},
		{
			date: "2026-07-27",
			engines: [
				{
					engine_id: "claude",
					input_tokens: 96_540,
					model_id: "claude-fable-5",
					output_tokens: 12_780,
				},
			],
			input_tokens: 96_540,
			output_tokens: 12_780,
		},
	],
	transcript: {
		status: "available",
		journal_sequence: 48,
		entries: [
			{
				event_id: "event-fixture-message",
				journal_sequence: 47,
				occurred_at: fixture_timestamp,
				payload: {
					type: "assistant.message_completed",
					message_id: "message-fixture",
					text: "The editor shell fixture is ready.",
				},
			},
		],
	},
	preview_assets: {
		a3c6b9aa8f1fc9f2443493ba4f997ba3d0623af616c9cbff797e51189d5b2c44: {
			asset_id: "a3c6b9aa8f1fc9f2443493ba4f997ba3d0623af616c9cbff797e51189d5b2c44",
			bytes: 10,
			content_type: "image/png",
		},
	} as Readonly<Record<string, RichLinkAssetMetadata>>,
	preview_targets: [
		{
			created_at: fixture_timestamp,
			id: "preview-artisan",
			journal_sequence: 48,
			launch_state: "idle",
			port: 5173,
			project_id: "project-artisan-editor",
			routes: ["/", "/visual-fixtures"],
			source: { kind: "terminal", terminal_id: "terminal-artisan" },
			state: "healthy",
			thread_id: "thread-artisan",
			updated_at: fixture_timestamp,
			url: "http://localhost:5173/",
			workspace_id: "workspace-artisan-editor",
		},
	],
	rich_links: {} as Readonly<Record<string, RichLinkResolution>>,
	terminal_output: {
		"terminal-editor-shell": new Uint8Array([
			112, 110, 112, 109, 32, 114, 117, 110, 32, 118, 97, 108, 105, 100, 97, 116, 101, 10,
		]),
	} as Readonly<Record<string, Uint8Array>>,
	terminals: [
		{
			args: [],
			cols: 120,
			created_at: fixture_timestamp,
			executable: "pwsh.exe",
			generation: 1,
			pid: 4242,
			rows: 32,
			state: "active",
			terminal_id: "terminal-editor-shell",
			thread_id: "thread-editor-shell",
			updated_at: fixture_timestamp,
			working_directory: fixture_project.root_path,
			workspace_id: "workspace-artisan-editor",
		},
	],
	thread_retention_policy: {
		enabled: true,
		inactivity_days: 7,
	},
	thread_work: {
		agent_id: "agent-sol",
		display_name: "Sol",
		engine_id: "codex",
		role: "Coordinator",
		run_id: "run-editor-shell",
		status: "running",
	},
	threads: [
		{
			activity_version: 8,
			affinity_version: 4,
			created_at: fixture_timestamp,
			current_goal: "Shape the three-pane editor shell",
			last_activity_at: fixture_timestamp,
			reader_activity_at: fixture_timestamp,
			linked_projects: [fixture_project],
			live_status: "Building frontend fixtures",
			metadata_version: 5,
			pinned: true,
			primary_project: fixture_project,
			project_affinity_scores: [
				{
					evidence: [{ count: 3, kind: "active_working_directory" }],
					project: fixture_project,
					score: 100,
				},
			],
			project_locked: true,
			thread_id: "thread-editor-shell",
			title: "Shape the editor shell",
			title_locked: false,
			title_source: "automatic",
			updated_at: fixture_timestamp,
		},
	],
	workspace_changes: [
		{
			after_identity: {
				algorithm: "sha256",
				byte_count: 29,
				content_hash: "ddf066ee341c060cca67ed6faa462602690b490d98c6fecdf607c447754e14bb",
			},
			agent_id: "agent-terra",
			before_identity: {
				algorithm: "sha256",
				byte_count: 30,
				content_hash: "7bc245364a34f5905e223f7cd0230d6ce53a1b74c389c593d725aa2ff856e918",
			},
			change_id: "change-fixture-runtime",
			created_at: fixture_timestamp,
			path: "modules/frontend/src/lib/fixture.ts",
			review_state: "needs_review",
			rollback_state: "available",
			run_id: "run-editor-shell",
			source_command_id: "command-fixture-replace",
			thread_id: "thread-editor-shell",
			updated_at: fixture_timestamp,
			version: 1,
			workspace_id: "workspace-artisan-editor",
		},
	],
	workspace_change_diffs: {
		"thread-editor-shell:change-fixture-runtime": {
			added_line_count: 1,
			after_identity: {
				algorithm: "sha256",
				byte_count: 29,
				content_hash: "ddf066ee341c060cca67ed6faa462602690b490d98c6fecdf607c447754e14bb",
			},
			before_identity: {
				algorithm: "sha256",
				byte_count: 30,
				content_hash: "7bc245364a34f5905e223f7cd0230d6ce53a1b74c389c593d725aa2ff856e918",
			},
			change_id: "change-fixture-runtime",
			context_lines: 3,
			format: "unified",
			format_version: 1,
			patch: "--- a/modules/frontend/src/lib/fixture.ts\n+++ b/modules/frontend/src/lib/fixture.ts\n@@ -1,1 +1,1 @@\n-export const fixture = false;\n+export const fixture = true;\n",
			patch_identity: {
				algorithm: "sha256",
				byte_count: 161,
				content_hash: "7f37d522d6e251ead86503baa4e1d32a4bc3f20d84706b26a5b24a5dbdfe216f",
			},
			path: "modules/frontend/src/lib/fixture.ts",
			removed_line_count: 1,
			thread_id: "thread-editor-shell",
			truncated: false,
			workspace_id: "workspace-artisan-editor",
		},
	},
	workspace_files: {
		"workspace-artisan-editor:modules/frontend/src/lib/fixture.ts": {
			content: "export const fixture = true;\n",
			identity: {
				algorithm: "sha256",
				byte_count: 29,
				content_hash: "ddf066ee341c060cca67ed6faa462602690b490d98c6fecdf607c447754e14bb",
			},
			path: "modules/frontend/src/lib/fixture.ts",
			workspace_id: "workspace-artisan-editor",
		},
	},
} satisfies FixtureArtisanClientData;
