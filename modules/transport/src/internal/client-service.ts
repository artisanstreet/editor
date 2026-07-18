import { Cause, Effect, Layer, Option, Queue, Ref, Stream } from "effect";

import {
	type CommandEnvelope,
	type GitDiffQueryEnvelope,
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
	type GitWorkspaceQueryEnvelope,
	type WorkspaceChangeListQueryEnvelope,
	type WorkspaceChangeDiffQueryEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceFileReadQueryEnvelope,
	type WorkspaceFileReplaceEnvelope,
	type GlobalGuidanceDriftResolutionEnvelope,
	type GlobalGuidanceQueryEnvelope,
	type GlobalGuidanceRetryEnvelope,
	type GlobalGuidanceSelectionEnvelope,
	type GlobalGuidanceUpdateEnvelope,
	type ModelBehaviourDriftResolutionEnvelope,
	type ModelBehaviourQueryEnvelope,
	type ModelBehaviourRetryEnvelope,
	type ModelBehaviourUpdateEnvelope,
	type OrchestrationGraphQueryEnvelope,
	type OrchestrationGroupListQueryEnvelope,
	type TerminalListQueryEnvelope,
	type ThreadListQueryEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type ThreadWorkQueryEnvelope,
	type ThreadTranscriptQueryEnvelope,
} from "@artisan/protocol";

import {
	ArtisanClient,
	type ArtisanGitDiffInput,
	type ArtisanGitIndexMutationInput,
	type ArtisanGitMutationResolveInput,
	type ArtisanGitWorkspaceInput,
	type ArtisanGlobalGuidanceDriftInput,
	type ArtisanGlobalGuidanceRetryInput,
	type ArtisanGlobalGuidanceSelectionInput,
	type ArtisanGlobalGuidanceUpdateInput,
	type ArtisanModelBehaviourDriftInput,
	type ArtisanModelBehaviourRetryInput,
	type ArtisanModelBehaviourUpdateInput,
	type ArtisanClientError,
	type ArtisanClientOptions,
	type ArtisanCommandInput,
	type ArtisanCommandReceipt,
	type ArtisanWorkspaceChangeListInput,
	type ArtisanWorkspaceChangeDiffInput,
	type ArtisanWorkspaceChangeReviewInput,
	type ArtisanWorkspaceChangeRollbackInput,
	type ArtisanWorkspaceFileReadInput,
	type ArtisanWorkspaceFileReplaceInput,
	type ArtisanThreadRetentionUpdateInput,
} from "../client-contract";
import { TransportRuntime } from "../transport-runtime";
import { client_error, validate_client_options } from "./client-common";
import { make_client_connection_lifecycle } from "./client-connection";
import { make_client_request_coordinator } from "./client-request-coordinator";
import { make_client_stream_channel } from "./client-stream-channel";
import { make_client_subscription_coordinator } from "./client-subscription-coordinator";

/** Builds the public client service from focused lifecycle coordinators. */
export function make_artisan_client_layer(input_options: ArtisanClientOptions = {}) {
	const options: Required<ArtisanClientOptions> = {
		error_capacity: input_options.error_capacity ?? 64,
		event_capacity: input_options.event_capacity ?? 256,
		max_pending_requests: input_options.max_pending_requests ?? 128,
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
			const connection = yield* make_client_connection_lifecycle(options.reconnect_delay_ms);

			const publish_error = (error: ArtisanClientError) =>
				Effect.sync(() => {
					Queue.offerUnsafe(errors, error);
				});
			const requests = yield* make_client_request_coordinator(
				options.max_pending_requests,
				connection.SendCurrent,
			);
			const subscriptions = yield* make_client_subscription_coordinator(
				options.event_capacity,
				options.subscription_capacity,
				connection.MakeTrace,
				runtime.MakeId,
				connection.SendCurrent,
				publish_error,
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
					const result = yield* requests.Request(envelope, "command.receipt");

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

			const list_threads = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: ThreadListQueryEnvelope = {
					...trace,
					kind: "thread.list.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "thread.list.query.result");

				return result.kind === "thread.list.query.result"
					? result.payload.threads
					: yield* Effect.die("thread list response narrowed incorrectly");
			});

			const read_workspace_file = (input: ArtisanWorkspaceFileReadInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceFileReadQueryEnvelope = {
						...trace,
						kind: "workspace.file.read.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.file.read.query.result",
					);

					return result.kind === "workspace.file.read.query.result"
						? result.payload
						: yield* Effect.die("workspace file read response narrowed incorrectly");
				});
			const list_workspace_changes = (input: ArtisanWorkspaceChangeListInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceChangeListQueryEnvelope = {
						...trace,
						kind: "workspace.change.list.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.change.list.query.result",
					);

					return result.kind === "workspace.change.list.query.result"
						? result.payload
						: yield* Effect.die("workspace change list response narrowed incorrectly");
				});
			const get_workspace_change_diff = (input: ArtisanWorkspaceChangeDiffInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceChangeDiffQueryEnvelope = {
						...trace,
						kind: "workspace.change.diff.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.change.diff.query.result",
					);

					return result.kind === "workspace.change.diff.query.result"
						? result.payload
						: yield* Effect.die("workspace change diff response narrowed incorrectly");
				});
			const get_git_workspace = (input: ArtisanGitWorkspaceInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GitWorkspaceQueryEnvelope = {
						...trace,
						kind: "git.workspace.query",
						payload: input,
					};
					const result = yield* requests.Request(envelope, "git.workspace.query.result");

					return result.kind === "git.workspace.query.result"
						? result.payload
						: yield* Effect.die("Git workspace response narrowed incorrectly");
				});
			const get_git_diff = (input: ArtisanGitDiffInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GitDiffQueryEnvelope = {
						...trace,
						kind: "git.diff.query",
						payload: input,
					};
					const result = yield* requests.Request(envelope, "git.diff.query.result");

					return result.kind === "git.diff.query.result"
						? result.payload
						: yield* Effect.die("Git diff response narrowed incorrectly");
				});

			type GitMutationEnvelope =
				| GitIndexStageRequestEnvelope
				| GitIndexUnstageRequestEnvelope
				| GitMutationResolveEnvelope;
			const send_git_mutation = (envelope: GitMutationEnvelope) =>
				Effect.gen(function* () {
					const result = yield* requests.Request(envelope, "command.receipt");

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("Git mutation receipt narrowed incorrectly");
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
						command_id: envelope.message_id,
						journal_sequence: result.payload.journal_sequence,
						status: result.payload.status,
					} satisfies ArtisanCommandReceipt;
				});
			const request_git_index_mutation = (input: ArtisanGitIndexMutationInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const message_id = input.command_id ?? trace.message_id;
					const mutation_id =
						input.mutation_id === undefined
							? yield* runtime.MakeId("git_mutation")
							: input.mutation_id;
					const approval_id =
						input.approval_id === undefined
							? yield* runtime.MakeId("git_approval")
							: input.approval_id;
					const payload = {
						approval_id,
						expected_snapshot_id: input.expected_snapshot_id,
						expected_workspace_version: input.expected_workspace_version,
						mutation_id,
						paths: input.paths,
						workspace_id: input.workspace_id,
					};
					const attribution = {
						...trace,
						message_id,
						thread_id: input.thread_id,
						...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
						...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
						...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					};
					const envelope: GitIndexStageRequestEnvelope | GitIndexUnstageRequestEnvelope =
						input.kind === "stage"
							? { ...attribution, kind: "git.index.stage.request", payload }
							: { ...attribution, kind: "git.index.unstage.request", payload };

					return yield* send_git_mutation(envelope);
				});
			const resolve_git_mutation = (input: ArtisanGitMutationResolveInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GitMutationResolveEnvelope = {
						...trace,
						kind: "git.mutation.resolve",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							approval_id: input.approval_id,
							approved: input.approved,
							mutation_id: input.mutation_id,
						},
						thread_id: input.thread_id,
						...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
						...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
						...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					};

					return yield* send_git_mutation(envelope);
				});

			type WorkspaceMutationEnvelope =
				| WorkspaceChangeReviewEnvelope
				| WorkspaceChangeRollbackEnvelope
				| WorkspaceFileReplaceEnvelope;
			const send_workspace_mutation = (envelope: WorkspaceMutationEnvelope) =>
				Effect.gen(function* () {
					const result = yield* requests.Request(envelope, "command.receipt");

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("workspace mutation receipt narrowed incorrectly");
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
						command_id: envelope.message_id,
						journal_sequence: result.payload.journal_sequence,
						status: result.payload.status,
					} satisfies ArtisanCommandReceipt;
				});
			const replace_workspace_file = (input: ArtisanWorkspaceFileReplaceInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceFileReplaceEnvelope = {
						...trace,
						agent_id: input.agent_id,
						kind: "workspace.file.replace",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							change_id: input.change_id,
							content: input.content,
							expected_before: input.expected_before,
							path: input.path,
							workspace_id: input.workspace_id,
						},
						run_id: input.run_id,
						thread_id: input.thread_id,
						...(input.raw_origin === undefined ? {} : { raw_origin: input.raw_origin }),
					};

					return yield* send_workspace_mutation(envelope);
				});
			const review_workspace_change = (input: ArtisanWorkspaceChangeReviewInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceChangeReviewEnvelope = {
						...trace,
						kind: "workspace.change.review",
						message_id: input.command_id ?? trace.message_id,
						payload: { change_id: input.change_id },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const rollback_workspace_change = (input: ArtisanWorkspaceChangeRollbackInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceChangeRollbackEnvelope = {
						...trace,
						kind: "workspace.change.rollback",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							change_id: input.change_id,
							expected_after: input.expected_after,
						},
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});

			const get_thread_retention_policy = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: ThreadRetentionQueryEnvelope = {
					...trace,
					kind: "thread.retention.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "thread.retention.query.result");

				return result.kind === "thread.retention.query.result"
					? result.payload
					: yield* Effect.die("thread retention response narrowed incorrectly");
			});

			const get_global_guidance = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: GlobalGuidanceQueryEnvelope = {
					...trace,
					kind: "guidance.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "guidance.query.result");

				return result.kind === "guidance.query.result"
					? result.payload
					: yield* Effect.die("global guidance response narrowed incorrectly");
			});

			type GuidanceMutationEnvelope =
				| GlobalGuidanceDriftResolutionEnvelope
				| GlobalGuidanceRetryEnvelope
				| GlobalGuidanceSelectionEnvelope
				| GlobalGuidanceUpdateEnvelope;
			const send_guidance_mutation = (envelope: GuidanceMutationEnvelope) =>
				Effect.gen(function* () {
					const result = yield* requests.Request(envelope, "command.receipt");

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("global guidance receipt narrowed incorrectly");
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
						command_id: envelope.message_id,
						journal_sequence: result.payload.journal_sequence,
						status: result.payload.status,
					} satisfies ArtisanCommandReceipt;
				});
			const update_global_guidance = (input: ArtisanGlobalGuidanceUpdateInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GlobalGuidanceUpdateEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "guidance.update",
						payload: { content: input.content },
					};

					return yield* send_guidance_mutation(envelope);
				});
			const select_global_guidance = (input: ArtisanGlobalGuidanceSelectionInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GlobalGuidanceSelectionEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "guidance.selection",
						payload: {
							content_hash: input.content_hash,
							provider: input.provider,
						},
					};

					return yield* send_guidance_mutation(envelope);
				});
			const resolve_global_guidance_drift = (input: ArtisanGlobalGuidanceDriftInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GlobalGuidanceDriftResolutionEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "guidance.drift.resolve",
						payload: {
							action: input.action,
							observed_hash: input.observed_hash,
							provider: input.provider,
						},
					};

					return yield* send_guidance_mutation(envelope);
				});
			const retry_global_guidance_sync = (input: ArtisanGlobalGuidanceRetryInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: GlobalGuidanceRetryEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "guidance.sync.retry",
						payload: { provider: input.provider },
					};

					return yield* send_guidance_mutation(envelope);
				});

			const get_model_behaviour = Effect.gen(function* () {
				const trace = yield* connection.MakeTrace;
				const envelope: ModelBehaviourQueryEnvelope = {
					...trace,
					kind: "model_behaviour.query",
					payload: {},
				};
				const result = yield* requests.Request(envelope, "model_behaviour.query.result");

				return result.kind === "model_behaviour.query.result"
					? result.payload
					: yield* Effect.die("Model Behaviour response narrowed incorrectly");
			});

			type ModelBehaviourMutationEnvelope =
				| ModelBehaviourDriftResolutionEnvelope
				| ModelBehaviourRetryEnvelope
				| ModelBehaviourUpdateEnvelope;
			const send_model_behaviour_mutation = (envelope: ModelBehaviourMutationEnvelope) =>
				Effect.gen(function* () {
					const result = yield* requests.Request(envelope, "command.receipt");

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("Model Behaviour receipt narrowed incorrectly");
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
						command_id: envelope.message_id,
						journal_sequence: result.payload.journal_sequence,
						status: result.payload.status,
					} satisfies ArtisanCommandReceipt;
				});
			const update_model_behaviour = (input: ArtisanModelBehaviourUpdateInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ModelBehaviourUpdateEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "model_behaviour.update",
						payload: {
							setting_id: input.setting_id,
							value: input.value,
						},
					};

					return yield* send_model_behaviour_mutation(envelope);
				});
			const resolve_model_behaviour_drift = (input: ArtisanModelBehaviourDriftInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ModelBehaviourDriftResolutionEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "model_behaviour.drift.resolve",
						payload: {
							action: input.action,
							observed_hash: input.observed_hash,
							provider_id: input.provider_id,
							setting_id: input.setting_id,
						},
					};

					return yield* send_model_behaviour_mutation(envelope);
				});
			const retry_model_behaviour_sync = (input: ArtisanModelBehaviourRetryInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ModelBehaviourRetryEnvelope = {
						...trace,
						message_id: input.command_id ?? trace.message_id,
						kind: "model_behaviour.sync.retry",
						payload: {
							provider_id: input.provider_id,
							setting_id: input.setting_id,
						},
					};

					return yield* send_model_behaviour_mutation(envelope);
				});

			const update_thread_retention_policy = (input: ArtisanThreadRetentionUpdateInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const command_id = input.command_id ?? trace.message_id;
					const envelope: ThreadRetentionUpdateEnvelope = {
						...trace,
						message_id: command_id,
						kind: "thread.retention.update",
						payload: {
							enabled: input.enabled,
							inactivity_days: input.inactivity_days,
						},
					};
					const result = yield* requests.Request(envelope, "command.receipt");

					if (result.kind !== "command.receipt") {
						return yield* Effect.die("thread retention receipt narrowed incorrectly");
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

			const get_thread_work = (thread_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ThreadWorkQueryEnvelope = {
						...trace,
						kind: "thread.work.query",
						payload: { thread_id },
					};
					const result = yield* requests.Request(envelope, "thread.work.query.result");

					return result.kind === "thread.work.query.result"
						? Option.fromUndefinedOr(result.payload.work)
						: yield* Effect.die("thread work response narrowed incorrectly");
				});

			const get_orchestration_graph = (group_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: OrchestrationGraphQueryEnvelope = {
						...trace,
						kind: "orchestration.graph.query",
						payload: { group_id },
					};
					const result = yield* requests.Request(
						envelope,
						"orchestration.graph.query.result",
					);

					return result.kind === "orchestration.graph.query.result"
						? result.payload.graph
						: yield* Effect.die("orchestration graph response narrowed incorrectly");
				});

			const get_thread_transcript = (
				input: import("@artisan/protocol").ThreadTranscriptQuery,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ThreadTranscriptQueryEnvelope = {
						...trace,
						kind: "thread.transcript.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"thread.transcript.query.result",
					);
					return result.kind === "thread.transcript.query.result"
						? result.payload
						: yield* Effect.die("thread transcript response narrowed incorrectly");
				});

			const list_orchestration_groups = (thread_id: string, include_terminal: boolean) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: OrchestrationGroupListQueryEnvelope = {
						...trace,
						kind: "orchestration.group.list.query",
						payload: { thread_id, include_terminal },
					};
					const result = yield* requests.Request(
						envelope,
						"orchestration.group.list.query.result",
					);
					return result.kind === "orchestration.group.list.query.result"
						? result.payload
						: yield* Effect.die(
								"orchestration group list response narrowed incorrectly",
							);
				});

			const list_terminals = (thread_id: string, workspace_id: string) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: TerminalListQueryEnvelope = {
						...trace,
						kind: "terminal.list.query",
						payload: { thread_id, workspace_id },
					};
					const result = yield* requests.Request(envelope, "terminal.list.query.result");

					return result.kind === "terminal.list.query.result"
						? result.payload.terminals
						: yield* Effect.die("terminal list response narrowed incorrectly");
				});

			return {
				Command: command,
				Cursors: subscriptions.Cursors,
				Dispose: shutdown(Option.none()),
				Errors: Stream.fromQueue(errors),
				Events: subscriptions.Events,
				GetOrchestrationGraph: get_orchestration_graph,
				GetThreadTranscript: get_thread_transcript,
				ListOrchestrationGroups: list_orchestration_groups,
				GetGlobalGuidance: get_global_guidance,
				GetGitDiff: get_git_diff,
				GetGitWorkspace: get_git_workspace,
				GetModelBehaviour: get_model_behaviour,
				GetThreadRetentionPolicy: get_thread_retention_policy,
				GetThreadWork: get_thread_work,
				GetWorkspaceChangeDiff: get_workspace_change_diff,
				ListWorkspaceChanges: list_workspace_changes,
				ListTerminals: list_terminals,
				ListThreads: list_threads,
				OpenAsset: (asset_id) => streams.Open(`asset:${asset_id}`),
				OpenTerminalOutput: (terminal_id) => streams.Open(`terminal:${terminal_id}`),
				ReadWorkspaceFile: read_workspace_file,
				ResolveGlobalGuidanceDrift: resolve_global_guidance_drift,
				RequestGitIndexMutation: request_git_index_mutation,
				ResolveGitMutation: resolve_git_mutation,
				ResolveModelBehaviourDrift: resolve_model_behaviour_drift,
				RetryGlobalGuidanceSync: retry_global_guidance_sync,
				RetryModelBehaviourSync: retry_model_behaviour_sync,
				ReplaceWorkspaceFile: replace_workspace_file,
				ReviewWorkspaceChange: review_workspace_change,
				RollbackWorkspaceChange: rollback_workspace_change,
				SelectGlobalGuidance: select_global_guidance,
				SubscribeOrchestrationGraph: subscriptions.SubscribeOrchestrationGraph,
				SubscribeOrchestrationGroups: subscriptions.SubscribeOrchestrationGroups,
				SubscribeThreadList: subscriptions.SubscribeThreadList,
				SubscribeThreadTranscript: subscriptions.SubscribeThreadTranscript,
				UpdateGlobalGuidance: update_global_guidance,
				UpdateModelBehaviour: update_model_behaviour,
				UpdateThreadRetentionPolicy: update_thread_retention_policy,
			};
		}),
	);
}
