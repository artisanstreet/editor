import { Effect, Option, Stream } from "effect";
import { model_manifest } from "@artisan/catalog";
import type { TerminalSession } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

import type { FixtureArtisanClientData } from "./data";
import * as FixtureData from "./data";
import { FixtureProjectIdentityQueries } from "./project-identity-queries";
import {
	FixtureConversation,
	FixtureFailure,
	FixturePreviewTarget,
	FixtureReceipt,
	fixture_artisan_client_data,
	fixture_project,
	fixture_timestamp,
} from "./support";

void FixtureData;

export const FixtureClientQueries = {
	Command: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? `fixture-command-${input.payload.type}`,
			);
		}),
	CreateThread: (input) =>
		Effect.gen(function* () {
			const template = fixture_artisan_client_data.threads.at(0);
			if (template === undefined) {
				return yield* Effect.die("Fixture thread data must not be empty.");
			}
			return {
				...template,
				thread_id: "thread-fixture-created",
				title: input.title,
			};
		}),
	ConnectionChanges: Stream.empty,
	ConnectionState: Effect.gen(function* () {
		return { phase: "ready" as const };
	}),
	Cursors: Effect.gen(function* () {
		return fixture_artisan_client_data.cursors;
	}),
	Dispose: Effect.gen(function* () {}),
	Errors: Stream.empty,
	Events: Stream.fromIterable(fixture_artisan_client_data.events),
	RetryConnection: Effect.gen(function* () {}),
	GetGlobalGuidance: Effect.gen(function* () {
		return fixture_artisan_client_data.global_guidance;
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
		return fixture_artisan_client_data.model_behaviour;
	}),
	GetPreviewAssetMetadata: (input) =>
		Effect.gen(function* () {
			const asset = fixture_artisan_client_data.preview_assets[input.asset_id];

			return asset === undefined
				? yield* FixtureFailure(`Unknown fixture preview asset: ${input.asset_id}`)
				: asset;
		}),
	GetPreviewTarget: (input) =>
		Effect.gen(function* () {
			return yield* FixturePreviewTarget(input);
		}),
	GetRoutineDetail: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace routine fixtures are unavailable.");
		}),
	GetCapabilityDetail: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
	GetCapabilityOAuthStatus: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace OAuth fixtures are unavailable.");
		}),
	GetOrchestrationGraph: (group_id) =>
		Effect.gen(function* () {
			if (group_id !== fixture_artisan_client_data.orchestration_graph.group.group_id) {
				return yield* FixtureFailure(`Unknown fixture group: ${group_id}`);
			}

			return fixture_artisan_client_data.orchestration_graph;
		}),
	GetConversation: ({ thread_id }) =>
		Effect.gen(function* () {
			return FixtureConversation(thread_id);
		}),
	GetMessageImageAttachment: () =>
		Effect.gen(function* () {
			return Option.none();
		}),
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
		Effect.gen(function* () {
			return {
				thread_id,
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
				auto_steer_enabled: true,
				policy: {
					engine_id: "codex" as const,
					reasoning_effort: "medium" as const,
					permission: "supervised",
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
			};
		}),
	ListSurfaceItems: (_input) =>
		Effect.gen(function* () {
			return {
				items: [],
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			};
		}),
	GetSurfaceUsageAggregate: (input) =>
		Effect.gen(function* () {
			return {
				aggregate: { scope: input.scope, scope_id: input.scope_id },
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			};
		}),
	GetSurfaceUsageDaily: (input) =>
		Effect.gen(function* () {
			return {
				buckets: fixture_artisan_client_data.surface_usage_daily.slice(-input.day_count),
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			};
		}),
	ListOrchestrationGroups: (thread_id, include_terminal) =>
		Effect.gen(function* () {
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
		return fixture_artisan_client_data.thread_retention_policy;
	}),
	GetThreadWork: (thread_id) =>
		Effect.gen(function* () {
			return thread_id === fixture_artisan_client_data.orchestration_graph.group.thread_id
				? Option.some(fixture_artisan_client_data.thread_work)
				: Option.none();
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
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Workspace language capabilities are unavailable in the frontend fixture.",
			);
		}),
	ListTerminals: (thread_id, workspace_id) =>
		Effect.gen(function* () {
			const terminals: Array<TerminalSession> = [];

			for (const terminal of fixture_artisan_client_data.terminals) {
				if (terminal.thread_id === thread_id && terminal.workspace_id === workspace_id) {
					terminals.push(terminal);
				}
			}

			return terminals;
		}),
	ListThreads: Effect.gen(function* () {
		return fixture_artisan_client_data.threads;
	}),
	ListProjects: Effect.gen(function* () {
		return {
			projects: [
				{
					...fixture_project,
					attached_at: fixture_timestamp,
					updated_at: fixture_timestamp,
				},
			],
		};
	}),
	GetModelFavorites: Effect.gen(function* () {
		return { model_ids: ["codex-sol"] };
	}),
	UpdateModelFavorite: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? `fixture-command-model-favorite-${input.model_id}`,
			);
		}),
	GetRuntimeCatalog: Effect.gen(function* () {
		return {
			default_model_id: "codex-sol",
			manifest: {
				...model_manifest,
				harnesses: model_manifest.harnesses.filter((harness) => harness.id === "codex"),
				models: model_manifest.models.filter((model) => model.harness === "codex"),
				providers: model_manifest.providers.filter((provider) => provider.id === "openai"),
			},
			runnable_harness_ids: ["codex" as const],
		};
	}),
	GetSessionDefaults: Effect.gen(function* () {
		return {
			last_model_id: "claude-sonnet-5",
			models: [
				{
					context_window: "[1m]",
					model_id: "claude-sonnet-5",
					reasoning_effort: "high" as const,
				},
			],
			permission: "supervised",
		};
	}),
	UpdateSessionDefaults: () =>
		Effect.gen(function* () {
			return {
				command_id: "command-session-defaults",
				journal_sequence: 1,
				status: "accepted" as const,
			};
		}),
	...FixtureProjectIdentityQueries,
	DetachProject: () =>
		Effect.gen(function* () {
			return { projects: [] };
		}),
	ListProjectDirectories: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Project directory browsing is unavailable in the frontend fixture.",
			);
		}),
	SelectProjectDirectory: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Project directory selection is unavailable in the frontend fixture.",
			);
		}),
	CreateProjectDirectory: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Project directory creation is unavailable in the frontend fixture.",
			);
		}),
	ListArtisanApprovals: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Artisan approvals are unavailable in the frontend fixture.",
			);
		}),
	ListArtisanToolInvocations: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Artisan tool invocations are unavailable in the frontend fixture.",
			);
		}),
	ListArtisanTools: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Artisan tool registry is unavailable in the frontend fixture.",
			);
		}),
	ListPreviewTargets: (input = {}) =>
		Effect.gen(function* () {
			return fixture_artisan_client_data.preview_targets.filter(
				(target) =>
					input.workspace_id === undefined || target.workspace_id === input.workspace_id,
			);
		}),
	ListRoutines: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace routine fixtures are unavailable.");
		}),
	ListCapabilities: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
} satisfies Pick<
	typeof ArtisanClient.Service,
	| "Command"
	| "CreateThread"
	| "ConnectionChanges"
	| "ConnectionState"
	| "Cursors"
	| "Dispose"
	| "Errors"
	| "Events"
	| "RetryConnection"
	| "GetGlobalGuidance"
	| "GetGitDiff"
	| "GetGitWorkspace"
	| "GetModelBehaviour"
	| "GetPreviewAssetMetadata"
	| "GetPreviewTarget"
	| "GetRoutineDetail"
	| "GetCapabilityDetail"
	| "GetCapabilityOAuthStatus"
	| "GetOrchestrationGraph"
	| "GetConversation"
	| "GetMessageImageAttachment"
	| "GetThreadTranscript"
	| "GetThreadSession"
	| "ListSurfaceItems"
	| "GetSurfaceUsageAggregate"
	| "GetSurfaceUsageDaily"
	| "ListOrchestrationGroups"
	| "GetThreadRetentionPolicy"
	| "GetThreadWork"
	| "GetWorkspaceChangeDiff"
	| "GetWorkspaceLanguageCapabilities"
	| "ListTerminals"
	| "ListThreads"
	| "ListProjects"
	| "GetModelFavorites"
	| "UpdateModelFavorite"
	| "GetRuntimeCatalog"
	| "GetSessionDefaults"
	| "UpdateSessionDefaults"
	| "GetProjectRepositories"
	| "GetProjectDiffs"
	| "GetHostIdentity"
	| "GetEngineUsage"
	| "DetachProject"
	| "ListProjectDirectories"
	| "SelectProjectDirectory"
	| "CreateProjectDirectory"
	| "ListArtisanApprovals"
	| "ListArtisanToolInvocations"
	| "ListArtisanTools"
	| "ListPreviewTargets"
	| "ListRoutines"
	| "ListCapabilities"
>;
