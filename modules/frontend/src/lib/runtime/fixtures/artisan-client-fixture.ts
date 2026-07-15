import { Effect, Layer, Option, Stream } from "effect";

import type {
	EventEnvelope,
	ExternalWaitQueryResult,
	GlobalGuidanceSnapshot,
	HostedGitSnapshotQueryResult,
	HostedGitCheckFailureDetailQueryResult,
	ModelBehaviourSnapshot,
	OrchestrationGraph,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
	WorkspaceChange,
	WorkspaceChangeDiffQueryResult,
	WorkspaceFileReadQueryResult,
	WorkspaceGitFetchQueryResult,
	WorkspaceGitMutationApproval,
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
	readonly model_behaviour: ModelBehaviourSnapshot;
	readonly orchestration_graph: OrchestrationGraph;
	readonly terminal_output: Readonly<Record<string, Uint8Array>>;
	readonly terminals: ReadonlyArray<TerminalSession>;
	readonly thread_retention_policy: ThreadRetentionPolicy;
	readonly thread_work: ThreadWorkItem;
	readonly threads: ReadonlyArray<ThreadListItem>;
	readonly workspace_changes: ReadonlyArray<WorkspaceChange>;
	readonly workspace_change_diffs: Readonly<Record<string, WorkspaceChangeDiffQueryResult>>;
	readonly workspace_files: Readonly<Record<string, WorkspaceFileReadQueryResult>>;
	readonly workspace_git_mutation_approval: WorkspaceGitMutationApproval;
}

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
	workspace_git_mutation_approval: {
		approval_id: "git-mutation-approval-editor-shell",
		created_at: fixture_timestamp,
		expected_session_version: 3,
		operation: { type: "commit" },
		source_branch: "main",
		source_command_id: "command-fixture-git-mutation",
		source_head: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
		state: "requested",
		thread_id: "thread-editor-shell",
		updated_at: fixture_timestamp,
		workspace_id: "workspace-artisan-editor",
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

/** Complete deterministic Artisan client service used only by fixture compositions. */
export const FixtureArtisanClientService = {
	Command: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? `fixture-command-${input.payload.type}`,
			);
		}),
	Cursors: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.cursors);
	}),
	Dispose: Effect.gen(function* () {
		return yield* Effect.void;
	}),
	Errors: Stream.empty,
	Events: Stream.fromIterable(fixture_artisan_client_data.events),
	GetGlobalGuidance: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.global_guidance);
	}),
	GetHostedGitSnapshot: (_input) =>
		Effect.succeed({
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		} satisfies HostedGitSnapshotQueryResult),
	GetHostedGitCheckFailureDetail: (_input) =>
		Effect.succeed({
			detail: {
				check_origin: {
					native_id: "fixture-check-run",
					provider_id: "github",
					resource_kind: "check_run",
				},
				head_commit: "a".repeat(40),
				log: { _tag: "unavailable", reason: "not_available" },
				name: "Fixture checks",
				output: {
					summary: {
						_tag: "available",
						truncated: false,
						untrusted_text: "Checks failed",
					},
					text: { _tag: "unavailable" },
				},
			},
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			observed_at: fixture_timestamp,
			snapshot_version: 1,
			workspace_id: "workspace_fixture",
		} satisfies HostedGitCheckFailureDetailQueryResult),
	GetModelBehaviour: Effect.gen(function* () {
		return yield* Effect.succeed(fixture_artisan_client_data.model_behaviour);
	}),
	GetOrchestrationGraph: (group_id) =>
		Effect.gen(function* () {
			if (group_id !== fixture_artisan_client_data.orchestration_graph.group.group_id) {
				return yield* FixtureFailure(`Unknown fixture group: ${group_id}`);
			}

			return fixture_artisan_client_data.orchestration_graph;
		}),
	GetHostedProjectCloneApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				`Unknown fixture hosted project clone approval: ${input.approval_id}`,
			);
		}),
	GetExternalWaits: (_input) =>
		Effect.succeed({
			snapshots: [],
			truncated: false,
		} satisfies ExternalWaitQueryResult),
	GetPreviewTargets: (input) =>
		Effect.succeed({
			project_id: input.project_id,
			targets: [],
			workspace_id: input.workspace_id,
		}),
	GetPreviewBrowserLifecycle: (input) =>
		Effect.succeed({
			inspections: [],
			launches: [],
			project_id: input.project_id,
			workspace_id: input.workspace_id,
		}),
	GetRichLinkMetadata: (input) =>
		Effect.succeed({
			cache: { expires_at_ms: 1_752_580_800_000, status: "miss" as const },
			fetched_at_ms: 1_752_580_500_000,
			final_url: input.url,
			page_name: "Artisan",
			requested_url: input.url,
			site_name: "Artisan",
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
	GetWorkspaceReplaceApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				`Unknown fixture workspace replacement approval: ${input.approval_id}`,
			);
		}),
	GetWorkspaceGitSession: (_input) =>
		Effect.succeed({
			journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
		}),
	GetWorkspaceGitFetch: Effect.succeed({
		enabled: false,
		workspaces: [],
	} satisfies WorkspaceGitFetchQueryResult),
	GetWorkspaceGitCheckoutApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				`Unknown fixture workspace Git checkout approval: ${input.approval_id}`,
			);
		}),
	GetWorkspaceGitMutationApproval: (input) =>
		Effect.gen(function* () {
			const approval = fixture_artisan_client_data.workspace_git_mutation_approval;

			if (
				input.approval_id !== approval.approval_id ||
				input.thread_id !== approval.thread_id
			) {
				return yield* FixtureFailure(
					`Unknown fixture workspace Git mutation approval: ${input.approval_id}`,
				);
			}

			return { approval };
		}),
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
	OpenAsset: (asset_id) =>
		Effect.gen(function* () {
			const output = fixture_artisan_client_data.asset_output[asset_id];

			if (output === undefined) {
				return yield* FixtureFailure(`Unknown fixture asset: ${asset_id}`);
			}

			return Stream.fromIterable([output]);
		}),
	OpenTerminalOutput: (terminal_id) =>
		Effect.gen(function* () {
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
	ResolveGlobalGuidanceDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-drift");
		}),
	ResolveModelBehaviourDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-drift");
		}),
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
	RespondWorkspaceReplaceApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-replace-approval");
		}),
	RefreshWorkspaceGitSession: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-git-refresh");
		}),
	RequestWorkspaceGitFetch: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-git-fetch");
		}),
	RefreshHostedGitSnapshot: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-hosted-git-refresh");
		}),
	RequestWorkspaceGitCheckout: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-git-checkout");
		}),
	RequestWorkspaceGitMutation: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-git-mutation");
		}),
	RequestHostedProjectClone: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-hosted-project-clone");
		}),
	RequestExternalWait: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-external-wait-request");
		}),
	CancelExternalWait: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-external-wait-cancel");
		}),
	ManuallyResumeExternalWait: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-external-wait-manual-resume");
		}),
	RegisterPreviewTarget: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-register");
		}),
	ProbePreviewTarget: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-probe");
		}),
	RemovePreviewTarget: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-remove");
		}),
	OpenPreviewInExternalBrowser: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-browser-open");
		}),
	AttachPreviewInspection: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-inspection-attach");
		}),
	DetachPreviewInspection: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-preview-inspection-detach");
		}),
	RespondWorkspaceGitCheckoutApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-workspace-git-checkout-approval",
			);
		}),
	RespondWorkspaceGitMutationApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-workspace-git-mutation-approval",
			);
		}),
	RespondHostedProjectCloneApproval: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-hosted-project-clone-approval",
			);
		}),
	SelectGlobalGuidance: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-select");
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
	UpdateGlobalGuidance: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-update");
		}),
	UpdateModelBehaviour: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-update");
		}),
	UpdateThreadRetentionPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-retention-update");
		}),
	UpdateWorkspaceGitFetchPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-workspace-git-fetch-policy");
		}),
} satisfies typeof ArtisanClient.Service;

/** Explicit test/visual Layer; production bootstraps must supply the live client Layer. */
export const FixtureArtisanClientLayer = Layer.succeed(ArtisanClient, FixtureArtisanClientService);
