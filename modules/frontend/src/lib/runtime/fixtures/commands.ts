import { Effect, Stream } from "effect";
import type { WorkspaceChange } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

import type { FixtureArtisanClientData } from "./data";
import * as FixtureData from "./data";
import { FixtureMarketplaceCommands } from "./marketplace-commands";
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

export const FixtureClientCommands = {
	ListWorkspaceChanges: (input) =>
		Effect.gen(function* () {
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
		Effect.gen(function* () {
			return {
				conflicts: [],
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
			};
		}),
	ListWorkspaceFiles: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure(
				"Workspace file discovery is unavailable in the frontend fixture.",
			);
		}),
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
		Effect.gen(function* () {
			return {
				closed_at: fixture_timestamp,
				connector_id: "fixture-browser",
				last_error: undefined,
				opened_at: fixture_timestamp,
				reconnect_state: "connected" as const,
				session_id,
				state: "closed" as const,
				target_id: "preview-artisan",
				updated_at: fixture_timestamp,
			};
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
	ProbePreviewTarget: (input) =>
		Effect.gen(function* () {
			return yield* FixturePreviewTarget(input);
		}),
	RegisterPreviewTarget: (input) =>
		Effect.gen(function* () {
			return {
				...input,
				created_at: fixture_timestamp,
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
				launch_state: "idle" as const,
				state: "registered" as const,
				updated_at: fixture_timestamp,
			};
		}),
	RemovePreviewTarget: (input) =>
		Effect.gen(function* () {
			const target = yield* FixturePreviewTarget(input);
			return {
				...target,
				state: "removed" as const,
				updated_at: fixture_timestamp,
			};
		}),
	...FixtureMarketplaceCommands,
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
		Effect.gen(function* () {
			return Stream.fromIterable([
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
			]);
		}),
	SubscribeThreadList: Effect.gen(function* () {
		return Stream.fromIterable([
			{
				journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
				threads: fixture_artisan_client_data.threads,
				type: "snapshot" as const,
			},
		]);
	}),
	SubscribeProjects: Effect.gen(function* () {
		return Stream.fromIterable([
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
		]);
	}),
	SubscribeConversation: (thread_id, _cursor?) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: FixtureConversation(thread_id),
				},
			]);
		}),
	SubscribeThreadTranscript: (thread_id) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
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
			]);
		}),
	SubscribeThreadSession: (thread_id) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
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
					},
				},
			]);
		}),
	SubscribeSurfaceItems: (_input) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						items: [],
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]);
		}),
	SubscribeSurfaceUsageAggregate: (input) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						aggregate: { scope: input.scope, scope_id: input.scope_id },
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]);
		}),
	SubscribeWorkspaceConflicts: (_thread_id) =>
		Effect.gen(function* () {
			return Stream.fromIterable([
				{
					type: "snapshot" as const,
					snapshot: {
						conflicts: [],
						journal_sequence: fixture_artisan_client_data.cursors.last_journal_sequence,
					},
				},
			]);
		}),
	UpdateGlobalGuidance: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-guidance-update");
		}),
} satisfies Pick<
	typeof ArtisanClient.Service,
	| "ListWorkspaceChanges"
	| "ListWorkspaceConflicts"
	| "ListWorkspaceFiles"
	| "OpenAsset"
	| "LaunchPreviewInExternalBrowser"
	| "OpenPreviewInspectionSession"
	| "InspectPreviewSession"
	| "ClosePreviewInspectionSession"
	| "OpenTerminalOutput"
	| "ReadWorkspaceFile"
	| "ReplaceWorkspaceFile"
	| "RequestGitIndexMutation"
	| "ExecuteArtisanTool"
	| "ResolveArtisanApproval"
	| "ResolveGitMutation"
	| "ResolveGlobalGuidanceDrift"
	| "ResolveModelBehaviourDrift"
	| "ResolveRichLink"
	| "ProbePreviewTarget"
	| "RegisterPreviewTarget"
	| "RemovePreviewTarget"
	| "PreviewRoutineInstall"
	| "RequestRoutineInstall"
	| "DecideRoutineInstall"
	| "EnableRoutine"
	| "DisableRoutine"
	| "RemoveRoutine"
	| "RollbackRoutine"
	| "SyncRoutine"
	| "ResolveRoutineDrift"
	| "RequestRoutineDriftOverwrite"
	| "DecideRoutineDriftOverwrite"
	| "InvokeRoutine"
	| "DiscoverNpxSkills"
	| "ImportNpxSkills"
	| "PreviewCapabilityConnect"
	| "RequestCapabilityConnect"
	| "DecideCapabilityConnect"
	| "StartCapability"
	| "ReconnectCapability"
	| "CheckCapabilityHealth"
	| "DisconnectCapability"
	| "RestartCapability"
	| "UninstallCapability"
	| "EnableCapability"
	| "DisableCapability"
	| "RemoveCapability"
	| "SyncCapability"
	| "ResolveCapabilityDrift"
	| "RequestCapabilityDriftOverwrite"
	| "DecideCapabilityDriftOverwrite"
	| "RequestCapabilityInvocation"
	| "DecideCapabilityInvocation"
	| "InvokeCapability"
	| "BeginCapabilityOAuth"
	| "CompleteCapabilityOAuth"
	| "RefreshCapabilityOAuth"
	| "RevokeCapabilityOAuth"
	| "RetryGlobalGuidanceSync"
	| "RetryModelBehaviourSync"
	| "ReviewWorkspaceChange"
	| "RollbackWorkspaceChange"
	| "SelectGlobalGuidance"
	| "SetPreviewTargetState"
	| "SubscribeOrchestrationGraph"
	| "SubscribeOrchestrationGroups"
	| "SubscribeThreadList"
	| "SubscribeProjects"
	| "SubscribeConversation"
	| "SubscribeThreadTranscript"
	| "SubscribeThreadSession"
	| "SubscribeSurfaceItems"
	| "SubscribeSurfaceUsageAggregate"
	| "SubscribeWorkspaceConflicts"
	| "UpdateGlobalGuidance"
>;
