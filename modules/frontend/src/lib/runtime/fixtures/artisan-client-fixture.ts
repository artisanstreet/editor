import { Effect, Layer, Option, Schema, Stream } from "effect";
import { model_manifest } from "@artisan/catalog";

import { ConversationSnapshot as ConversationSnapshotSchema } from "@artisan/protocol";
import type {
	ConversationSnapshot,
	EventEnvelope,
	GlobalGuidanceSnapshot,
	GitWorkspaceQueryResult,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	OrchestrationGroupListSnapshot,
	SurfaceUsageDailyBucket,
	PreviewTarget,
	PreviewTargetGetQuery,
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
import {
	ArtisanClient,
	ArtisanClientError,
	type ArtisanClientCursors,
} from "@artisan/transport/client";

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

export const FixtureConversation = (thread_id: string): ConversationSnapshot => {
	const created_at = "2026-07-24T00:00:00.000Z";
	const Entity = (
		id: string,
		ordinal: number,
		lifecycle: "active" | "completed" = "completed",
	) => ({
		created_at,
		id,
		lifecycle,
		ordinal,
		references: [],
		revision: 1,
		source_refs: [{ provider: "fixture", reference: `fixture:${id}` }],
		updated_at: created_at,
	});
	const turn_ids = ["turn-1", "turn-2", "turn-3", "turn-4"] as const;
	const turn_ordinals = [0, 6, 14, 19] as const;

	return Schema.decodeUnknownSync(ConversationSnapshotSchema)({
		conversation_id: `conversation:${thread_id}`,
		items: [
			{
				...Entity("message-user-1", 1),
				text: "Can you make the thread transcript stable while tools, reasoning, and file changes stream in?",
				turn_id: turn_ids[0],
				type: "user_message",
			},
			{
				...Entity("reasoning-1", 2),
				text: "I’ll first map the existing event flow, then introduce one deterministic conversation projection rather than asking each component to infer state.",
				turn_id: turn_ids[0],
				type: "reasoning_summary",
			},
			{
				...Entity("work-1", 3),
				ended_at: created_at,
				started_at: created_at,
				status: "completed",
				title: "Mapped the engine and transcript pipeline",
				turn_id: turn_ids[0],
				type: "work_session",
			},
			{
				...Entity("activity-1", 4),
				detail: "Read the normalizer, journal, transport, and renderer boundaries.",
				kind: "read",
				label: "Inspected conversation flow",
				status: "completed",
				turn_id: turn_ids[0],
				type: "activity",
			},
			{
				...Entity("message-assistant-1", 5),
				phase: "final",
				text: "The brittle part was not rendering itself. Several layers were independently guessing how provider events belonged together. I’ve reduced that to one typed stream with stable identities.",
				turn_id: turn_ids[0],
				type: "assistant_message",
			},
			{
				...Entity("message-user-2", 7),
				text: "Good. Make changed files and work sessions first-class instead of parsing headings.",
				turn_id: turn_ids[1],
				type: "user_message",
			},
			{
				...Entity("work-2", 8),
				ended_at: created_at,
				started_at: created_at,
				status: "completed",
				title: "Built the canonical conversation reducer",
				turn_id: turn_ids[1],
				type: "work_session",
			},
			{
				...Entity("activity-2", 9),
				detail: "Added ordered turns, messages, work sessions, activities, and changes.",
				kind: "write",
				label: "Defined renderer-ready entities",
				status: "completed",
				turn_id: turn_ids[1],
				type: "activity",
			},
			{
				...Entity("change-set-1", 10),
				file_count: 3,
				file_ids: ["file-protocol", "file-projection", "file-renderer"],
				state: "applied",
				summary: "Added the conversation protocol, durable projection, and typed renderer",
				turn_id: turn_ids[1],
				type: "change_set",
			},
			{
				...Entity("file-change-1", 11),
				change_set_id: "change-set-1",
				diff: { additions: 127, deletions: 8, kind: "known" },
				operation: "created",
				path: "modules/protocol/src/conversation.ts",
				turn_id: turn_ids[1],
				type: "file_change",
			},
			{
				...Entity("file-change-2", 12),
				change_set_id: "change-set-1",
				diff: { additions: 42, deletions: 3, kind: "known" },
				operation: "created",
				path: "modules/backend/src/conversation/projection.ts",
				turn_id: turn_ids[1],
				type: "file_change",
			},
			{
				...Entity("message-assistant-2", 13),
				phase: "final",
				text: "Changed files now arrive as explicit change-set and file-change entities. “Worked for” is a work-session lifecycle, so neither relies on timing or text heuristics.",
				turn_id: turn_ids[1],
				type: "assistant_message",
			},
			{
				...Entity("message-user-3", 15),
				text: "What happens if the stream reconnects or sends the same patch twice?",
				turn_id: turn_ids[2],
				type: "user_message",
			},
			{
				...Entity("reasoning-2", 16),
				text: "The frontend should apply only contiguous revisions. A gap, identity mismatch, or illegal lifecycle transition must trigger a snapshot resync instead of producing a half-valid UI.",
				turn_id: turn_ids[2],
				type: "reasoning_summary",
			},
			{
				...Entity("activity-3", 17),
				detail: "Replayed duplicate, delayed, and out-of-order patches against the reducer.",
				kind: "test",
				label: "Exercised recovery behavior",
				status: "completed",
				turn_id: turn_ids[2],
				type: "activity",
			},
			{
				...Entity("message-assistant-3", 18),
				phase: "final",
				text: "Duplicate patch IDs are idempotent. Sequence gaps and invalid transitions request a clean snapshot, while completed entities remain immutable. The transcript no longer flickers between interpretations.",
				turn_id: turn_ids[2],
				type: "assistant_message",
			},
			{
				...Entity("message-user-4", 20),
				text: "Make sure the mock is long enough to judge scrolling and the sticky composer.",
				turn_id: turn_ids[3],
				type: "user_message",
			},
			{
				...Entity("work-3", 21, "active"),
				started_at: created_at,
				status: "active",
				title: "Rendering the deterministic thread fixture",
				turn_id: turn_ids[3],
				type: "work_session",
			},
			{
				...Entity("activity-4", 22, "active"),
				detail: "Populating the mock through the same protocol schema used by live threads.",
				kind: "render",
				label: "Prepared visual fixture",
				status: "active",
				turn_id: turn_ids[3],
				type: "activity",
			},
			{
				...Entity("message-assistant-4", 23),
				phase: "commentary",
				text: "The mock now covers enough distinct entity types and vertical space to inspect transcript scrolling, grouping, status treatments, and the sticky glass composer without inventing a second UI-only data shape.",
				turn_id: turn_ids[3],
				type: "assistant_message",
			},
		],
		journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		last_patch_sequence: 0,
		schema_version: 1,
		thread_id,
		turns: turn_ids.map((id, index) => ({
			...Entity(
				id,
				turn_ordinals[index] ?? index,
				index === turn_ids.length - 1 ? "active" : "completed",
			),
			type: "turn",
		})),
		updated_at: created_at,
	});
};

const fixture_timestamp = "2026-07-12T10:00:00.000Z";

const fixture_project = {
	display_name: "Artisan Editor",
	project_id: "project-artisan-editor",
	root_path: "C:\\Users\\Sander\\Desktop\\artisan-editor",
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
		{ date: "2026-07-24", input_tokens: 48_120, output_tokens: 6_430 },
		{ date: "2026-07-25", input_tokens: 131_890, output_tokens: 18_240 },
		{ date: "2026-07-26", input_tokens: 22_470, output_tokens: 3_110 },
		{ date: "2026-07-27", input_tokens: 96_540, output_tokens: 12_780 },
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

const FixtureFailure = (message: string) =>
	Effect.gen(function* () {
		return yield* Effect.fail(
			new ArtisanClientError({
				cause: undefined,
				code: "protocol",
				message,
				protocol_code: "fixture_not_found",
				retryable: false,
			}),
		);
	});

const FixtureReceipt = (command_id: string, journal_sequence = 48) =>
	Effect.gen(function* () {
		return yield* Effect.succeed({
			command_id,
			journal_sequence,
			status: "accepted" as const,
		});
	});

const FixturePreviewTarget = (input: PreviewTargetGetQuery) =>
	Effect.gen(function* () {
		const target = fixture_artisan_client_data.preview_targets.find(
			(candidate) => candidate.id === input.target_id,
		);

		return target === undefined
			? yield* FixtureFailure(`Unknown fixture preview target: ${input.target_id}`)
			: target;
	});

/** Complete deterministic Artisan client service used only by fixture compositions. */
export const FixtureArtisanClientService = {
	Command: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? `fixture-command-${input.payload.type}`,
			);
		}),
	CreateThread: (input) =>
		Effect.succeed({
			...fixture_artisan_client_data.threads[0]!,
			thread_id: "thread-fixture-created",
			title: input.title,
		}),
	ConnectionChanges: Stream.empty,
	ConnectionState: Effect.succeed({ phase: "ready" as const }),
	Cursors: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.cursors);
	}),
	Dispose: Effect.gen(function* () {
		return yield* Effect.void;
	}),
	Errors: Stream.empty,
	Events: Stream.fromIterable(fixture_artisan_client_data.events),
	RetryConnection: Effect.void,
	GetGlobalGuidance: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.global_guidance);
	}),
	GetGitDiff: (input) =>
		Effect.gen(function* () {
			const workspace = fixture_artisan_client_data.git_workspace.workspace;

			if (
				input.workspace_id !== workspace.workspace_id ||
				input.expected_snapshot_id !== workspace.snapshot_id ||
				input.expected_workspace_version !== workspace.version
			) {
				return yield* FixtureFailure(`Unknown fixture Git snapshot: ${input.workspace_id}`);
			}

			return {
				byte_count: 0,
				format: "unified" as const,
				format_version: 1 as const,
				patch: "",
				scope: input.scope,
				snapshot_id: workspace.snapshot_id,
				truncated: false,
				workspace_id: workspace.workspace_id,
				workspace_version: workspace.version,
			};
		}),
	GetGitWorkspace: (input) =>
		Effect.gen(function* () {
			if (
				input.workspace_id !==
				fixture_artisan_client_data.git_workspace.workspace.workspace_id
			) {
				return yield* FixtureFailure(
					`Unknown fixture Git workspace: ${input.workspace_id}`,
				);
			}

			return fixture_artisan_client_data.git_workspace;
		}),
	GetModelBehaviour: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.model_behaviour);
	}),
	GetPreviewAssetMetadata: (input) =>
		Effect.gen(function* () {
			const asset = fixture_artisan_client_data.preview_assets[input.asset_id];

			return asset === undefined
				? yield* FixtureFailure(`Unknown fixture preview asset: ${input.asset_id}`)
				: asset;
		}),
	GetPreviewTarget: (input) => FixturePreviewTarget(input),
	GetRoutineDetail: () => FixtureFailure("Marketplace routine fixtures are unavailable."),
	GetCapabilityDetail: () => FixtureFailure("Marketplace capability fixtures are unavailable."),
	GetCapabilityOAuthStatus: () => FixtureFailure("Marketplace OAuth fixtures are unavailable."),
	GetOrchestrationGraph: (group_id) =>
		Effect.gen(function* () {
			if (group_id !== fixture_artisan_client_data.orchestration_graph.group.group_id) {
				return yield* FixtureFailure(`Unknown fixture group: ${group_id}`);
			}

			return fixture_artisan_client_data.orchestration_graph;
		}),
	GetConversation: ({ thread_id }) => Effect.succeed(FixtureConversation(thread_id)),
	GetMessageImageAttachment: () => Effect.succeed(Option.none()),
	GetThreadTranscript: (input) =>
		Effect.gen(function* () {
			if (input.thread_id !== "thread-editor-shell")
				return yield* FixtureFailure(`Unknown fixture thread: ${input.thread_id}`);
			const transcript = fixture_artisan_client_data.transcript;
			if (transcript.status !== "available") return transcript;
			const limit = input.limit ?? 100;
			const matching = transcript.entries.filter(
				(entry) =>
					(input.after_journal_sequence === undefined ||
						entry.journal_sequence > input.after_journal_sequence) &&
					(input.before_journal_sequence === undefined ||
						entry.journal_sequence < input.before_journal_sequence),
			);
			const entries =
				input.after_journal_sequence === undefined
					? matching.slice(-limit)
					: matching.slice(0, limit);
			return {
				...transcript,
				entries,
				...(entries.length === limit && entries[0] !== undefined
					? { next_before_journal_sequence: entries[0].journal_sequence }
					: {}),
			};
		}),
	GetThreadSession: (thread_id) =>
		Effect.succeed({
			thread_id,
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			auto_steer_enabled: true,
			policy: {
				engine_id: "codex" as const,
				reasoning_effort: "medium" as const,
				permission_mode: "on_request" as const,
				sandbox_mode: "workspace_write" as const,
				service_tier: "standard" as const,
				web_search_enabled: false,
				strict_clarification: false,
			},
			latest_intake: {
				message_id: "message-fixture",
				risk: "low" as const,
				resolution: "proceed" as const,
			},
			assumptions: [],
			last_routing: {
				type: "thread.message_routed" as const,
				message_id: "message-fixture",
				outcome: "queued" as const,
				reason: "no_active_run" as const,
				run_id: "run-editor-shell",
			},
		}),
	ListSurfaceItems: (_input) =>
		Effect.succeed({
			items: [],
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		}),
	GetSurfaceUsageAggregate: (input) =>
		Effect.succeed({
			aggregate: { scope: input.scope, scope_id: input.scope_id },
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		}),
	GetSurfaceUsageDaily: (input) =>
		Effect.succeed({
			buckets: fixture_artisan_client_data.surface_usage_daily.slice(-input.day_count),
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		}),
	ListOrchestrationGroups: (thread_id, include_terminal) =>
		Effect.sync(() => {
			if (thread_id !== "thread-editor-shell")
				return {
					groups: [],
					journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
				};
			return {
				...fixture_artisan_client_data.orchestration_groups,
				groups: fixture_artisan_client_data.orchestration_groups.groups.filter(
					(group) =>
						include_terminal ||
						!["summarized", "stopped", "failed", "complete"].includes(group.state),
				),
			};
		}),
	GetThreadRetentionPolicy: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.thread_retention_policy);
	}),
	GetThreadWork: (thread_id) =>
		Effect.gen(function* () {
			return yield* Effect.succeed(
				thread_id === fixture_artisan_client_data.orchestration_graph.group.thread_id
					? Option.some(fixture_artisan_client_data.thread_work)
					: Option.none(),
			);
		}),
	GetWorkspaceChangeDiff: (input) =>
		Effect.gen(function* () {
			const key = `${input.thread_id}:${input.change_id}`;
			const workspace_change_diffs: FixtureArtisanClientData["workspace_change_diffs"] =
				fixture_artisan_client_data.workspace_change_diffs;
			const diff = workspace_change_diffs[key];

			if (diff === undefined) {
				return yield* FixtureFailure(`Unknown fixture workspace change diff: ${key}`);
			}

			return diff;
		}),
	GetWorkspaceLanguageCapabilities: () =>
		FixtureFailure("Workspace language capabilities are unavailable in the frontend fixture."),
	ListTerminals: (thread_id, workspace_id) =>
		Effect.gen(function* () {
			const terminals: Array<TerminalSession> = [];

			for (const terminal of fixture_artisan_client_data.terminals) {
				if (terminal.thread_id === thread_id && terminal.workspace_id === workspace_id) {
					terminals.push(terminal);
				}
			}

			return yield* Effect.succeed(terminals);
		}),
	ListThreads: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.threads);
	}),
	ListProjects: Effect.succeed({
		projects: [
			{
				...fixture_project,
				attached_at: fixture_timestamp,
				updated_at: fixture_timestamp,
			},
		],
	}),
	GetRuntimeCatalog: Effect.succeed({
		default_model_id: "codex-sol",
		manifest: {
			...model_manifest,
			harnesses: model_manifest.harnesses.filter((harness) => harness.id === "codex"),
			models: model_manifest.models.filter((model) => model.harness === "codex"),
			providers: model_manifest.providers.filter((provider) => provider.id === "openai"),
		},
	}),
	DetachProject: () => Effect.succeed({ projects: [] }),
	ListProjectDirectories: () =>
		FixtureFailure("Project directory browsing is unavailable in the frontend fixture."),
	SelectProjectDirectory: () =>
		FixtureFailure("Project directory selection is unavailable in the frontend fixture."),
	ListArtisanApprovals: () =>
		FixtureFailure("Artisan approvals are unavailable in the frontend fixture."),
	ListArtisanToolInvocations: () =>
		FixtureFailure("Artisan tool invocations are unavailable in the frontend fixture."),
	ListArtisanTools: () =>
		FixtureFailure("Artisan tool registry is unavailable in the frontend fixture."),
	ListPreviewTargets: (input = {}) =>
		Effect.gen(function* () {
			yield* Effect.void;

			return fixture_artisan_client_data.preview_targets.filter(
				(target) =>
					input.workspace_id === undefined || target.workspace_id === input.workspace_id,
			);
		}),
	ListRoutines: () => FixtureFailure("Marketplace routine fixtures are unavailable."),
	ListCapabilities: () => FixtureFailure("Marketplace capability fixtures are unavailable."),
	ListWorkspaceChanges: (input) =>
		Effect.gen(function* () {
			yield* Effect.void;

			const changes: Array<WorkspaceChange> = [];

			for (const change of fixture_artisan_client_data.workspace_changes) {
				if (
					change.thread_id === input.thread_id &&
					(input.workspace_id === undefined || change.workspace_id === input.workspace_id)
				) {
					changes.push(change);
				}
			}

			return {
				changes,
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			};
		}),
	ListWorkspaceConflicts: (_thread_id) =>
		Effect.succeed({
			conflicts: [],
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		}),
	ListWorkspaceFiles: () =>
		FixtureFailure("Workspace file discovery is unavailable in the frontend fixture."),
	OpenAsset: (asset_id) =>
		Effect.gen(function* () {
			const output = fixture_artisan_client_data.asset_output[asset_id];

			if (output === undefined) {
				return yield* FixtureFailure(`Unknown fixture asset: ${asset_id}`);
			}

			return Stream.fromIterable([output]);
		}),
	LaunchPreviewInExternalBrowser: (input) =>
		Effect.gen(function* () {
			yield* FixturePreviewTarget(input);

			return { launched_at: fixture_timestamp, target_id: input.target_id };
		}),
	OpenPreviewInspectionSession: (input) =>
		Effect.gen(function* () {
			yield* FixturePreviewTarget({ target_id: input.target_id });

			return {
				connector_id: input.connector_id,
				opened_at: fixture_timestamp,
				reconnect_state: "connected" as const,
				session_id: `fixture-inspection-${input.target_id}`,
				state: "open" as const,
				target_id: input.target_id,
				updated_at: fixture_timestamp,
			};
		}),
	InspectPreviewSession: (input) =>
		Effect.gen(function* () {
			const target = yield* FixturePreviewTarget({ target_id: "preview-artisan" });

			return input.operation === "health"
				? {
						health: {
							checked_at: fixture_timestamp,
							latency_ms: 4,
							status: "healthy" as const,
							status_code: 200,
						},
						operation: "health" as const,
						session_id: input.session_id,
					}
				: { operation: "metadata" as const, session_id: input.session_id, target };
		}),
	ClosePreviewInspectionSession: (session_id) =>
		Effect.succeed({
			closed_at: fixture_timestamp,
			connector_id: "fixture-browser",
			last_error: undefined,
			opened_at: fixture_timestamp,
			reconnect_state: "connected" as const,
			session_id,
			state: "closed" as const,
			target_id: "preview-artisan",
			updated_at: fixture_timestamp,
		}),
	OpenTerminalOutput: ({ terminal_id, thread_id, workspace_id }) =>
		Effect.gen(function* () {
			const terminal = fixture_artisan_client_data.terminals.find(
				(candidate) =>
					candidate.terminal_id === terminal_id &&
					candidate.thread_id === thread_id &&
					candidate.workspace_id === workspace_id,
			);
			if (terminal === undefined)
				return yield* FixtureFailure(`Unknown fixture terminal scope: ${terminal_id}`);
			const output = fixture_artisan_client_data.terminal_output[terminal_id];

			if (output === undefined) {
				return yield* FixtureFailure(`Unknown fixture terminal: ${terminal_id}`);
			}

			return Stream.fromIterable([output]);
		}),
	ReadWorkspaceFile: (input) =>
		Effect.gen(function* () {
			const key = `${input.workspace_id}:${input.path}`;
			const workspace_files: FixtureArtisanClientData["workspace_files"] =
				fixture_artisan_client_data.workspace_files;
			const file = workspace_files[key];

			if (file === undefined) {
				return yield* FixtureFailure(`Unknown fixture workspace file: ${key}`);
			}

			return file;
		}),
	ReplaceWorkspaceFile: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-replace");
		}),
	RequestGitIndexMutation: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-git-index-mutation");
		}),
	ExecuteArtisanTool: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-artisan-tool-execute");
		}),
	ResolveArtisanApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-artisan-approval-resolve");
		}),
	ResolveGitMutation: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-git-mutation-resolve");
		}),
	ResolveGlobalGuidanceDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-drift");
		}),
	ResolveModelBehaviourDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-drift");
		}),
	ResolveRichLink: (input) =>
		Effect.gen(function* () {
			const resolved = fixture_artisan_client_data.rich_links[input.url];

			return resolved === undefined
				? yield* FixtureFailure(`Unknown fixture rich link: ${input.url}`)
				: resolved;
		}),
	ProbePreviewTarget: (input) => FixturePreviewTarget(input),
	RegisterPreviewTarget: (input) =>
		Effect.succeed({
			...input,
			created_at: fixture_timestamp,
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			launch_state: "idle" as const,
			state: "registered" as const,
			updated_at: fixture_timestamp,
		}),
	RemovePreviewTarget: (input) =>
		FixturePreviewTarget(input).pipe(
			Effect.map((target) => ({
				...target,
				state: "removed" as const,
				updated_at: fixture_timestamp,
			})),
		),
	PreviewRoutineInstall: () => FixtureFailure("Marketplace routine fixtures are unavailable."),
	RequestRoutineInstall: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-install"),
	DecideRoutineInstall: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-decision"),
	EnableRoutine: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-enable"),
	DisableRoutine: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-disable"),
	RemoveRoutine: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-remove"),
	RollbackRoutine: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-rollback"),
	SyncRoutine: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-sync"),
	ResolveRoutineDrift: (input) => FixtureReceipt(input.command_id ?? "fixture-routine-drift"),
	RequestRoutineDriftOverwrite: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-routine-drift-overwrite-request"),
	DecideRoutineDriftOverwrite: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-routine-drift-overwrite-decision"),
	InvokeRoutine: () => FixtureFailure("Marketplace routine fixtures are unavailable."),
	DiscoverNpxSkills: () => FixtureFailure("Marketplace npx-skills fixtures are unavailable."),
	ImportNpxSkills: (input) => FixtureReceipt(input.command_id ?? "fixture-npx-skills-import"),
	PreviewCapabilityConnect: () =>
		FixtureFailure("Marketplace capability fixtures are unavailable."),
	RequestCapabilityConnect: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-connect"),
	DecideCapabilityConnect: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-decision"),
	StartCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-start"),
	ReconnectCapability: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-reconnect"),
	CheckCapabilityHealth: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-health"),
	DisconnectCapability: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-disconnect"),
	RestartCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-restart"),
	UninstallCapability: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-uninstall"),
	EnableCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-enable"),
	DisableCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-disable"),
	RemoveCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-remove"),
	SyncCapability: (input) => FixtureReceipt(input.command_id ?? "fixture-capability-sync"),
	ResolveCapabilityDrift: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-drift"),
	RequestCapabilityDriftOverwrite: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-drift-overwrite-request"),
	DecideCapabilityDriftOverwrite: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-drift-overwrite-decision"),
	RequestCapabilityInvocation: () =>
		FixtureFailure("Marketplace capability fixtures are unavailable."),
	DecideCapabilityInvocation: () =>
		FixtureFailure("Marketplace capability fixtures are unavailable."),
	InvokeCapability: () => FixtureFailure("Marketplace capability fixtures are unavailable."),
	BeginCapabilityOAuth: () =>
		Effect.succeed({
			authorization_url: "https://fixture.invalid/oauth/authorize",
			continuation_reference: "fixture-oauth-continuation",
		}),
	CompleteCapabilityOAuth: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-oauth-complete"),
	RefreshCapabilityOAuth: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-oauth-refresh"),
	RevokeCapabilityOAuth: (input) =>
		FixtureReceipt(input.command_id ?? "fixture-capability-oauth-revoke"),
	RetryGlobalGuidanceSync: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-retry");
		}),
	RetryModelBehaviourSync: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-retry");
		}),
	ReviewWorkspaceChange: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-review");
		}),
	RollbackWorkspaceChange: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-rollback");
		}),
	SelectGlobalGuidance: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-select");
		}),
	SetPreviewTargetState: (input) =>
		Effect.gen(function* () {
			const current = yield* FixturePreviewTarget({
				target_id: input.target_id,
			});

			return { ...current, state: input.state, updated_at: fixture_timestamp };
		}),
	SubscribeOrchestrationGraph: (group_id) =>
		Effect.gen(function* () {
			if (group_id !== fixture_artisan_client_data.orchestration_graph.group.group_id) {
				return yield* FixtureFailure(`Unknown fixture group: ${group_id}`);
			}

			return Stream.fromIterable([
				{
					graph: fixture_artisan_client_data.orchestration_graph,
					journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					type: "snapshot" as const,
				},
			]);
		}),
	SubscribeOrchestrationGroups: (thread_id, include_terminal) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot:
						thread_id === "thread-editor-shell"
							? {
									...fixture_artisan_client_data.orchestration_groups,
									groups: fixture_artisan_client_data.orchestration_groups.groups.filter(
										(group) =>
											include_terminal ||
											![
												"summarized",
												"stopped",
												"failed",
												"complete",
											].includes(group.state),
									),
								}
							: {
									groups: [],
									journal_sequence:
										fixture_artisan_client_data.cursors.last_journal_sequence,
								},
				},
			]),
		),
	SubscribeThreadList: Effect.gen(function* () {
		return yield* Effect.succeed(
			Stream.fromIterable([
				{
					journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					threads: fixture_artisan_client_data.threads,
					type: "snapshot" as const,
				},
			]),
		);
	}),
	SubscribeProjects: Effect.succeed(
		Stream.fromIterable([
			{
				snapshot: {
					projects: [
						{
							...fixture_project,
							attached_at: fixture_timestamp,
							updated_at: fixture_timestamp,
						},
					],
				},
				type: "snapshot" as const,
			},
		]),
	),
	SubscribeConversation: (thread_id) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: FixtureConversation(thread_id),
				},
			]),
		),
	SubscribeThreadTranscript: (thread_id) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					transcript:
						thread_id === "thread-editor-shell"
							? fixture_artisan_client_data.transcript
							: {
									status: "unavailable" as const,
									journal_sequence:
										fixture_artisan_client_data.cursors.last_journal_sequence,
									entries: [],
								},
				},
			]),
		),
	SubscribeThreadSession: (thread_id) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						thread_id,
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
						auto_steer_enabled: true,
						policy: {
							engine_id: "codex" as const,
							reasoning_effort: "medium" as const,
							permission_mode: "on_request" as const,
							sandbox_mode: "workspace_write" as const,
							service_tier: "standard" as const,
							web_search_enabled: false,
							strict_clarification: false,
						},
						latest_intake: {
							message_id: "message-fixture",
							risk: "low" as const,
							resolution: "proceed" as const,
						},
						assumptions: [],
						last_routing: {
							type: "thread.message_routed" as const,
							message_id: "message-fixture",
							outcome: "queued" as const,
							reason: "no_active_run" as const,
							run_id: "run-editor-shell",
						},
					},
				},
			]),
		),
	SubscribeSurfaceItems: (_input) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						items: [],
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]),
		),
	SubscribeSurfaceUsageAggregate: (input) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						aggregate: { scope: input.scope, scope_id: input.scope_id },
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]),
		),
	SubscribeWorkspaceConflicts: (_thread_id) =>
		Effect.succeed(
			Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						conflicts: [],
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]),
		),
	UpdateGlobalGuidance: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-update");
		}),
	UpdateModelBehaviour: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-update");
		}),
	UpdateThreadSessionPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-session-policy-update");
		}),
	UpdateThreadRetentionPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-retention-update");
		}),
} satisfies typeof ArtisanClient.Service;

/** Explicit test/visual Layer; production bootstraps must supply the live client Layer. */
export const FixtureArtisanClientLayer = Layer.succeed(ArtisanClient, FixtureArtisanClientService);
