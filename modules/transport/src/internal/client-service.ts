import { Cause, Effect, Layer, Option, Queue, Ref, Stream } from "effect";

import type { CommandEnvelope } from "@artisan/protocol";

import {
	ArtisanClient,
	type ArtisanClientError,
	type ArtisanClientOptions,
	type ArtisanCommandInput,
	type ArtisanCommandReceipt,
} from "../client-api/service";
import { TransportRuntime } from "../transport-runtime";
import { client_error, validate_client_options } from "./client-common";
import { make_client_connection_lifecycle } from "./client-connection";
import { make_client_request_coordinator } from "./client-request-coordinator";
import { make_client_stream_channel } from "./client-stream-channel";
import { MakeClientApi } from "./api/context";
import { MakeCapabilityApi } from "./api/capabilities";
import { MakeGitMutationApi } from "./api/git-mutations";
import { MakeGuidanceApi } from "./api/guidance";
import { MakePreviewApi } from "./api/preview";
import { MakeQueryApi } from "./api/queries";
import { MakeRoutineApi } from "./api/routines";
import { MakeToolMutationApi } from "./api/tools";
import { MakeThreadOperationsApi } from "./api/thread-operations";
import { MakeWorkspaceApi } from "./api/workspace";
import { make_client_subscription_coordinator } from "./subscriptions/coordinator";
import {
	SubscriptionErrorReporter,
	SubscriptionIdentity,
	SubscriptionOptions,
	SubscriptionProtocol,
} from "./subscriptions/context";

export function make_artisan_client_layer(input_options: ArtisanClientOptions = {}) {
	const options: Required<ArtisanClientOptions> = {
		error_capacity: input_options.error_capacity ?? 64,
		event_capacity: input_options.event_capacity ?? 256,
		max_pending_requests: input_options.max_pending_requests ?? 128,
		reconnect_attempts: input_options.reconnect_attempts ?? 5,
		reconnect_delay_ms: input_options.reconnect_delay_ms ?? 50,
		stream_capacity: input_options.stream_capacity ?? 64,
		subscription_capacity: input_options.subscription_capacity ?? 64,
	};

	return Layer.effect(
		ArtisanClient,
		Effect.gen(function* () {
			if (!validate_client_options(options)) {
				return yield* Effect.fail(
					client_error(
						"configuration",
						"Artisan client limits are invalid.",
						new Error("client limits must be bounded safe integers"),
					),
				);
			}

			const runtime = yield* TransportRuntime;
			const errors = yield* Effect.acquireRelease(
				Queue.dropping<ArtisanClientError, Cause.Done<void>>(options.error_capacity),
				Queue.shutdown,
			);
			const disposed = yield* Ref.make(false);
			const connection = yield* make_client_connection_lifecycle(
				options.reconnect_delay_ms,
				options.reconnect_attempts,
			);

			const publish_error = (error: ArtisanClientError) =>
				Effect.sync(() => {
					Queue.offerUnsafe(errors, error);
				});
			const requests = yield* make_client_request_coordinator(
				options.max_pending_requests,
				connection.SendRequest,
			);
			const subscription_layer = Layer.mergeAll(
				Layer.succeed(SubscriptionOptions, {
					event_capacity: options.event_capacity,
					subscription_capacity: options.subscription_capacity,
				}),
				Layer.succeed(SubscriptionIdentity, {
					make_id: runtime.MakeId,
					make_trace: connection.MakeTrace,
				}),
				Layer.succeed(SubscriptionProtocol, {
					send_current: connection.SendCurrent,
				}),
				Layer.succeed(SubscriptionErrorReporter, {
					publish_error,
				}),
			);
			const subscriptions = yield* make_client_subscription_coordinator.pipe(
				Effect.provide(subscription_layer),
			);
			const streams = yield* make_client_stream_channel(
				options.stream_capacity,
				runtime.MakeId,
				connection.AwaitActive,
				connection.Current,
			);

			const shutdown = (failure: Option.Option<ArtisanClientError>) =>
				Effect.uninterruptible(
					Effect.gen(function* () {
						const should_close = yield* Ref.getAndSet(disposed, true);

						if (should_close) {
							return;
						}

						const terminal_error = Option.getOrElse(failure, () =>
							client_error(
								"disposed",
								"The Artisan client was disposed.",
								new Error("client disposed"),
							),
						);

						yield* requests.Dispose(terminal_error);
						yield* subscriptions.Dispose(failure);
						yield* streams.Dispose(failure);
						yield* connection.Dispose;

						if (Option.isSome(failure)) {
							yield* publish_error(failure.value);
						}

						yield* Queue.end(errors);
					}),
				);

			yield* Effect.addFinalizer(() => shutdown(Option.none()));
			yield* connection.Start({
				on_fatal: (error) => shutdown(Option.some(error)),
				publish_error,
				requests,
				streams,
				subscriptions,
			});

			const command = (input: ArtisanCommandInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const command_id = input.command_id ?? trace.message_id;
					const envelope: CommandEnvelope = {
						...trace,
						message_id: command_id,
						kind: "command",
						payload: input.payload,
						thread_id: input.thread_id,
						...(input.agent_id ? { agent_id: input.agent_id } : {}),
						...(input.causation_id ? { causation_id: input.causation_id } : {}),
						...(input.run_id ? { run_id: input.run_id } : {}),
					};
					const result = yield* requests.Request(envelope);

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("command response narrowed incorrectly");
					}

					if (result.payload.status === "rejected") {
						return yield* Effect.fail(
							client_error(
								"protocol",
								result.payload.error.message,
								result.payload.error,
								result.payload.error.retryable,
								result.payload.error.code,
							),
						);
					}

					return {
						command_id,
						journal_sequence: result.payload.journal_sequence,
						status: result.payload.status,
					} satisfies ArtisanCommandReceipt;
				});

			const query_api = yield* MakeClientApi(MakeQueryApi, connection, requests, runtime);
			const {
				create_project_directory,
				create_thread,
				detach_project,
				get_thread_usage_series,
				get_engine_usage,
				get_host_identity,
				get_project_diffs,
				get_project_repositories,
				get_runtime_catalog,
				list_artisan_approvals,
				list_artisan_tool_invocations,
				list_artisan_tools,
				list_project_directories,
				list_projects,
				list_threads,
				select_project_directory,
			} = query_api;

			const workspace_api = yield* MakeClientApi(
				MakeWorkspaceApi,
				connection,
				requests,
				runtime,
			);
			const preview_api = yield* MakeClientApi(MakePreviewApi, connection, requests, runtime);
			const git_mutation_api = yield* MakeClientApi(
				MakeGitMutationApi,
				connection,
				requests,
				runtime,
			);
			const tool_mutation_api = yield* MakeClientApi(
				MakeToolMutationApi,
				connection,
				requests,
				runtime,
			);
			const thread_operations_api = yield* MakeClientApi(
				MakeThreadOperationsApi,
				connection,
				requests,
				runtime,
			);
			const routine_api = yield* MakeClientApi(MakeRoutineApi, connection, requests, runtime);
			const guidance_api = yield* MakeClientApi(
				MakeGuidanceApi,
				connection,
				requests,
				runtime,
			);
			const capability_api = yield* MakeClientApi(
				MakeCapabilityApi,
				connection,
				requests,
				runtime,
			);
			return {
				Command: command,
				ConnectionChanges: connection.ConnectionChanges,
				ConnectionState: connection.ConnectionState,
				Cursors: subscriptions.Cursors,
				Dispose: shutdown(Option.none()),
				Errors: Stream.fromQueue(errors),
				Events: subscriptions.Events,
				RetryConnection: connection.RetryConnection,
				GetConversation: thread_operations_api.get_conversation,
				GetMessageImageAttachment: thread_operations_api.get_message_image_attachment,
				GetOrchestrationGraph: thread_operations_api.get_orchestration_graph,
				GetThreadTranscript: thread_operations_api.get_thread_transcript,
				GetThreadSession: thread_operations_api.get_thread_session,
				ListSurfaceItems: thread_operations_api.list_surface_items,
				GetSurfaceUsageAggregate: thread_operations_api.get_surface_usage_aggregate,
				GetSurfaceUsageDaily: thread_operations_api.get_surface_usage_daily,
				ListOrchestrationGroups: thread_operations_api.list_orchestration_groups,
				GetGlobalGuidance: guidance_api.get_global_guidance,
				GetGitDiff: workspace_api.get_git_diff,
				GetGitWorkspace: workspace_api.get_git_workspace,
				GetModelBehaviour: guidance_api.get_model_behaviour,
				ListArtisanApprovals: list_artisan_approvals,
				ListArtisanToolInvocations: list_artisan_tool_invocations,
				ListArtisanTools: list_artisan_tools,
				GetPreviewAssetMetadata: preview_api.get_preview_asset_metadata,
				GetPreviewTarget: preview_api.get_preview_target,
				GetRoutineDetail: routine_api.get_routine_detail,
				GetCapabilityDetail: capability_api.get_capability_detail,
				GetCapabilityOAuthStatus: capability_api.get_capability_oauth_status,
				GetModelFavorites: thread_operations_api.get_model_favorites,
				GetThreadRetentionPolicy: thread_operations_api.get_thread_retention_policy,
				GetThreadWork: thread_operations_api.get_thread_work,
				CreateThread: create_thread,
				GetWorkspaceChangeDiff: workspace_api.get_workspace_change_diff,
				GetWorkspaceLanguageCapabilities: workspace_api.get_workspace_language_capabilities,
				ListWorkspaceChanges: workspace_api.list_workspace_changes,
				ListWorkspaceFiles: workspace_api.list_workspace_files,
				ListWorkspaceConflicts: workspace_api.list_workspace_conflicts,
				ListTerminals: thread_operations_api.list_terminals,
				ListThreads: list_threads,
				ListProjects: list_projects,
				GetRuntimeCatalog: get_runtime_catalog,
				GetHostIdentity: get_host_identity,
				GetThreadUsageSeries: get_thread_usage_series,
				GetEngineUsage: get_engine_usage,
				GetProjectDiffs: get_project_diffs,
				GetProjectRepositories: get_project_repositories,
				GetSessionDefaults: thread_operations_api.get_session_defaults,
				UpdateSessionDefaults: thread_operations_api.update_session_defaults,
				DetachProject: detach_project,
				ListProjectDirectories: list_project_directories,
				SelectProjectDirectory: select_project_directory,
				CreateProjectDirectory: create_project_directory,
				ListPreviewTargets: preview_api.list_preview_targets,
				ListRoutines: routine_api.list_routines,
				ListCapabilities: capability_api.list_capabilities,
				OpenAsset: (asset_id) => streams.Open(`asset:${asset_id}`),
				LaunchPreviewInExternalBrowser: preview_api.launch_preview_in_external_browser,
				OpenPreviewInspectionSession: preview_api.open_preview_inspection_session,
				InspectPreviewSession: preview_api.inspect_preview_session,
				ClosePreviewInspectionSession: preview_api.close_preview_inspection_session,
				OpenTerminalOutput: ({ terminal_id, thread_id, workspace_id }) =>
					streams.Open(
						`terminal:${encodeURIComponent(thread_id)}:${encodeURIComponent(workspace_id)}:${encodeURIComponent(terminal_id)}`,
					),
				ReadWorkspaceFile: workspace_api.read_workspace_file,
				ResolveGlobalGuidanceDrift: guidance_api.resolve_global_guidance_drift,
				RequestGitIndexMutation: git_mutation_api.request_git_index_mutation,
				ResolveGitMutation: git_mutation_api.resolve_git_mutation,
				ResolveArtisanApproval: tool_mutation_api.resolve_artisan_approval,
				ResolveModelBehaviourDrift: guidance_api.resolve_model_behaviour_drift,
				ResolveRichLink: preview_api.resolve_rich_link,

				PreviewRoutineInstall: routine_api.preview_routine_install,
				RequestRoutineInstall: routine_api.request_routine_install,
				DecideRoutineInstall: routine_api.decide_routine_install,
				EnableRoutine: (input) =>
					routine_api.routine_lifecycle(input, "marketplace.routine.enable"),
				DisableRoutine: (input) =>
					routine_api.routine_lifecycle(input, "marketplace.routine.disable"),
				RemoveRoutine: (input) =>
					routine_api.routine_lifecycle(input, "marketplace.routine.remove"),
				RollbackRoutine: routine_api.rollback_routine,
				SyncRoutine: routine_api.sync_routine,
				ResolveRoutineDrift: routine_api.resolve_routine_drift,
				RequestRoutineDriftOverwrite: routine_api.request_routine_drift_overwrite,
				DecideRoutineDriftOverwrite: routine_api.decide_routine_drift_overwrite,
				InvokeRoutine: routine_api.invoke_routine,
				DiscoverNpxSkills: routine_api.discover_npx_skills,
				ImportNpxSkills: routine_api.import_npx_skills,
				PreviewCapabilityConnect: capability_api.preview_capability_connect,
				RequestCapabilityConnect: capability_api.request_capability_connect,
				DecideCapabilityConnect: capability_api.decide_capability_connect,
				StartCapability: (input) =>
					capability_api.capability_lifecycle(input, "marketplace.capability.start"),
				ReconnectCapability: (input) =>
					capability_api.capability_lifecycle(input, "marketplace.capability.reconnect"),
				CheckCapabilityHealth: capability_api.check_capability_health,
				DisconnectCapability: (input) =>
					capability_api.capability_lifecycle(input, "marketplace.capability.disconnect"),
				RestartCapability: (input) =>
					capability_api.capability_lifecycle(input, "marketplace.capability.restart"),
				UninstallCapability: (input) =>
					capability_api.capability_lifecycle(input, "marketplace.capability.uninstall"),
				EnableCapability: (input) =>
					capability_api.capability_enablement(input, "marketplace.capability.enable"),
				DisableCapability: (input) =>
					capability_api.capability_enablement(input, "marketplace.capability.disable"),
				RemoveCapability: (input) =>
					capability_api.capability_enablement(input, "marketplace.capability.remove"),
				SyncCapability: capability_api.sync_capability,
				ResolveCapabilityDrift: capability_api.resolve_capability_drift,
				RequestCapabilityDriftOverwrite: capability_api.request_capability_drift_overwrite,
				DecideCapabilityDriftOverwrite: capability_api.decide_capability_drift_overwrite,
				RequestCapabilityInvocation: capability_api.request_capability_invocation,
				DecideCapabilityInvocation: capability_api.decide_capability_invocation,
				InvokeCapability: capability_api.invoke_capability,
				BeginCapabilityOAuth: capability_api.begin_capability_oauth,
				CompleteCapabilityOAuth: capability_api.complete_capability_oauth,
				RefreshCapabilityOAuth: (input) =>
					capability_api.capability_oauth_mutation(
						input,
						"marketplace.capability.oauth.refresh",
					),
				RevokeCapabilityOAuth: (input) =>
					capability_api.capability_oauth_mutation(
						input,
						"marketplace.capability.oauth.revoke",
					),
				RetryGlobalGuidanceSync: guidance_api.retry_global_guidance_sync,
				RetryModelBehaviourSync: guidance_api.retry_model_behaviour_sync,
				ProbePreviewTarget: preview_api.probe_preview_target,
				RegisterPreviewTarget: preview_api.register_preview_target,
				RemovePreviewTarget: preview_api.remove_preview_target,
				ReplaceWorkspaceFile: workspace_api.replace_workspace_file,
				ExecuteArtisanTool: tool_mutation_api.execute_artisan_tool,
				ReviewWorkspaceChange: workspace_api.review_workspace_change,
				RollbackWorkspaceChange: workspace_api.rollback_workspace_change,
				SelectGlobalGuidance: guidance_api.select_global_guidance,
				SetPreviewTargetState: preview_api.set_preview_target_state,
				SubscribeOrchestrationGraph: subscriptions.SubscribeOrchestrationGraph,
				SubscribeOrchestrationGroups: subscriptions.SubscribeOrchestrationGroups,
				SubscribeConversation: subscriptions.SubscribeConversation,
				SubscribeThreadList: subscriptions.SubscribeThreadList,
				SubscribeProjects: subscriptions.SubscribeProjects,
				SubscribeThreadTranscript: subscriptions.SubscribeThreadTranscript,
				SubscribeThreadSession: subscriptions.SubscribeThreadSession,
				SubscribeSurfaceItems: subscriptions.SubscribeSurfaceItems,
				SubscribeSurfaceUsageAggregate: subscriptions.SubscribeSurfaceUsageAggregate,
				SubscribeWorkspaceConflicts: subscriptions.SubscribeWorkspaceConflicts,
				UpdateGlobalGuidance: guidance_api.update_global_guidance,
				UpdateModelBehaviour: guidance_api.update_model_behaviour,
				UpdateModelFavorite: thread_operations_api.update_model_favorite,
				UpdateThreadRetentionPolicy: thread_operations_api.update_thread_retention_policy,
				UpdateThreadSessionPolicy: thread_operations_api.update_thread_session_policy,
			};
		}),
	);
}
