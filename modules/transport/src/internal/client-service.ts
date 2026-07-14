import { Cause, Effect, Layer, Option, Queue, Ref, Schema, Stream } from "effect";

import {
	type CommandEnvelope,
	type ExternalWaitCancelEnvelope,
	type ExternalWaitManualResumeEnvelope,
	type ExternalWaitQueryEnvelope,
	type ExternalWaitRequestEnvelope,
	type HostedProjectCloneApprovalQueryEnvelope,
	type HostedProjectCloneApprovalRespondEnvelope,
	HostedProjectCloneRequest,
	type HostedProjectCloneRequestEnvelope,
	type HostedGitSnapshotQueryEnvelope,
	type HostedGitSnapshotRefreshEnvelope,
	type WorkspaceChangeListQueryEnvelope,
	type WorkspaceChangeDiffQueryEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceReplaceApprovalQuery,
	type WorkspaceReplaceApprovalQueryEnvelope,
	type WorkspaceReplaceApprovalRespondEnvelope,
	type WorkspaceFileReadQueryEnvelope,
	type WorkspaceFileReplaceEnvelope,
	type WorkspaceGitCheckoutApprovalQuery,
	type WorkspaceGitCheckoutApprovalQueryEnvelope,
	type WorkspaceGitCheckoutApprovalRespondEnvelope,
	type WorkspaceGitCheckoutRequestEnvelope,
	type WorkspaceGitMutationApprovalQuery,
	type WorkspaceGitMutationApprovalQueryEnvelope,
	type WorkspaceGitMutationApprovalRespondEnvelope,
	WorkspaceGitMutationRequest,
	type WorkspaceGitMutationRequestEnvelope,
	type WorkspaceGitSessionQueryEnvelope,
	type WorkspaceGitSessionRefreshEnvelope,
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
	type TerminalListQueryEnvelope,
	type ThreadListQueryEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type ThreadWorkQueryEnvelope,
} from "@artisan/protocol";

import {
	ArtisanClient,
	type ArtisanExternalWaitCancelInput,
	type ArtisanExternalWaitManualResumeInput,
	type ArtisanExternalWaitQueryInput,
	type ArtisanExternalWaitRequestInput,
	type ArtisanHostedProjectCloneApprovalInput,
	type ArtisanHostedProjectCloneApprovalResponseInput,
	type ArtisanHostedProjectCloneInput,
	type ArtisanHostedGitSnapshotInput,
	type ArtisanHostedGitSnapshotRefreshInput,
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
	type ArtisanWorkspaceGitCheckoutApprovalResponseInput,
	type ArtisanWorkspaceGitCheckoutInput,
	type ArtisanWorkspaceGitSessionInput,
	type ArtisanWorkspaceGitSessionRefreshInput,
	type ArtisanWorkspaceGitMutationApprovalResponseInput,
	type ArtisanWorkspaceGitMutationInput,
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
			const get_workspace_replace_approval = (input: WorkspaceReplaceApprovalQuery) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceReplaceApprovalQueryEnvelope = {
						...trace,
						kind: "workspace.replace.approval.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.replace.approval.query.result",
					);

					return result.kind === "workspace.replace.approval.query.result"
						? result.payload
						: yield* Effect.die(
								"workspace replacement approval response narrowed incorrectly",
							);
				});
			const get_workspace_git_session = (input: ArtisanWorkspaceGitSessionInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitSessionQueryEnvelope = {
						...trace,
						kind: "workspace.git.session.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.git.session.query.result",
					);

					return result.kind === "workspace.git.session.query.result"
						? result.payload
						: yield* Effect.die("workspace Git session response narrowed incorrectly");
				});
			const get_hosted_git_snapshot = (input: ArtisanHostedGitSnapshotInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: HostedGitSnapshotQueryEnvelope = {
						...trace,
						kind: "hosted.git.snapshot.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"hosted.git.snapshot.query.result",
					);

					return result.kind === "hosted.git.snapshot.query.result"
						? result.payload
						: yield* Effect.die("hosted Git snapshot response narrowed incorrectly");
				});
			const get_workspace_git_checkout_approval = (
				input: WorkspaceGitCheckoutApprovalQuery,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitCheckoutApprovalQueryEnvelope = {
						...trace,
						kind: "workspace.git.checkout.approval.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.git.checkout.approval.query.result",
					);

					return result.kind === "workspace.git.checkout.approval.query.result"
						? result.payload
						: yield* Effect.die(
								"workspace Git checkout approval response narrowed incorrectly",
							);
				});
			const get_workspace_git_mutation_approval = (
				input: WorkspaceGitMutationApprovalQuery,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitMutationApprovalQueryEnvelope = {
						...trace,
						kind: "workspace.git.mutation.approval.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"workspace.git.mutation.approval.query.result",
					);

					return result.kind === "workspace.git.mutation.approval.query.result"
						? result.payload
						: yield* Effect.die(
								"workspace Git mutation approval response narrowed incorrectly",
							);
				});
			const get_hosted_project_clone_approval = (
				input: ArtisanHostedProjectCloneApprovalInput,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: HostedProjectCloneApprovalQueryEnvelope = {
						...trace,
						kind: "hosted.project.clone.approval.query",
						payload: input,
					};
					const result = yield* requests.Request(
						envelope,
						"hosted.project.clone.approval.query.result",
					);

					return result.kind === "hosted.project.clone.approval.query.result"
						? result.payload
						: yield* Effect.die(
								"hosted project clone approval response narrowed incorrectly",
							);
				});
			const get_external_waits = (input: ArtisanExternalWaitQueryInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ExternalWaitQueryEnvelope = {
						...trace,
						kind: "external_wait.query",
						payload: input,
					};
					const result = yield* requests.Request(envelope, "external_wait.query.result");

					return result.kind === "external_wait.query.result"
						? result.payload
						: yield* Effect.die("external wait query response narrowed incorrectly");
				});

			type WorkspaceMutationEnvelope =
				| HostedProjectCloneApprovalRespondEnvelope
				| HostedProjectCloneRequestEnvelope
				| WorkspaceChangeReviewEnvelope
				| WorkspaceChangeRollbackEnvelope
				| WorkspaceReplaceApprovalRespondEnvelope
				| WorkspaceGitCheckoutApprovalRespondEnvelope
				| WorkspaceGitCheckoutRequestEnvelope
				| WorkspaceGitMutationApprovalRespondEnvelope
				| WorkspaceGitMutationRequestEnvelope
				| WorkspaceGitSessionRefreshEnvelope
				| HostedGitSnapshotRefreshEnvelope
				| ExternalWaitCancelEnvelope
				| ExternalWaitManualResumeEnvelope
				| ExternalWaitRequestEnvelope
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
			const request_external_wait = (input: ArtisanExternalWaitRequestInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ExternalWaitRequestEnvelope = {
						...trace,
						kind: "external_wait.request",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							expected_head_commit: input.expected_head_commit,
							gates: input.gates,
							pull_request_number: input.pull_request_number,
							source_run_id: input.source_run_id,
							workspace_id: input.workspace_id,
						},
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const cancel_external_wait = (input: ArtisanExternalWaitCancelInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ExternalWaitCancelEnvelope = {
						...trace,
						kind: "external_wait.cancel",
						message_id: input.command_id ?? trace.message_id,
						payload: { wait_id: input.wait_id },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const manually_resume_external_wait = (input: ArtisanExternalWaitManualResumeInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: ExternalWaitManualResumeEnvelope = {
						...trace,
						kind: "external_wait.manual_resume",
						message_id: input.command_id ?? trace.message_id,
						payload: { wait_id: input.wait_id },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
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
							...(input.approval_request === undefined
								? {}
								: { approval_request: input.approval_request }),
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
			const respond_workspace_replace_approval = (input: {
				readonly approval_id: string;
				readonly approved: boolean;
				readonly command_id?: string;
				readonly thread_id: string;
			}) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceReplaceApprovalRespondEnvelope = {
						...trace,
						kind: "workspace.replace.approval.respond",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							approval_id: input.approval_id,
							approved: input.approved,
						},
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const refresh_workspace_git_session = (input: ArtisanWorkspaceGitSessionRefreshInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitSessionRefreshEnvelope = {
						...trace,
						kind: "workspace.git.session.refresh",
						message_id: input.command_id ?? trace.message_id,
						payload: { workspace_id: input.workspace_id },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const refresh_hosted_git_snapshot = (input: ArtisanHostedGitSnapshotRefreshInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: HostedGitSnapshotRefreshEnvelope = {
						...trace,
						kind: "hosted.git.snapshot.refresh",
						message_id: input.command_id ?? trace.message_id,
						payload: { workspace_id: input.workspace_id },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const request_workspace_git_checkout = (input: ArtisanWorkspaceGitCheckoutInput) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitCheckoutRequestEnvelope = {
						...trace,
						kind: "workspace.git.checkout.request",
						message_id: input.command_id ?? trace.message_id,
						payload: {
							expected_session_version: input.expected_session_version,
							target_branch: input.target_branch,
							workspace_id: input.workspace_id,
						},
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const respond_workspace_git_checkout_approval = (
				input: ArtisanWorkspaceGitCheckoutApprovalResponseInput,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitCheckoutApprovalRespondEnvelope = {
						...trace,
						kind: "workspace.git.checkout.approval.respond",
						message_id: input.command_id ?? trace.message_id,
						payload: { approval_id: input.approval_id, approved: input.approved },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const request_workspace_git_mutation = (input: ArtisanWorkspaceGitMutationInput) =>
				Effect.gen(function* () {
					const payload = yield* Schema.decodeUnknownEffect(WorkspaceGitMutationRequest, {
						onExcessProperty: "error",
					})({
						...(input.action_approval_id === undefined
							? {}
							: { action_approval_id: input.action_approval_id }),
						expected_session_version: input.expected_session_version,
						operation: input.operation,
						workspace_id: input.workspace_id,
					}).pipe(
						Effect.mapError((cause) =>
							client_error(
								"malformed",
								"The Git mutation request is invalid.",
								cause,
							),
						),
					);
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitMutationRequestEnvelope = {
						...trace,
						kind: "workspace.git.mutation.request",
						message_id: input.command_id ?? trace.message_id,
						payload,
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const respond_workspace_git_mutation_approval = (
				input: ArtisanWorkspaceGitMutationApprovalResponseInput,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: WorkspaceGitMutationApprovalRespondEnvelope = {
						...trace,
						kind: "workspace.git.mutation.approval.respond",
						message_id: input.command_id ?? trace.message_id,
						payload: { approval_id: input.approval_id, approved: input.approved },
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const request_hosted_project_clone = (input: ArtisanHostedProjectCloneInput) =>
				Effect.gen(function* () {
					const payload = yield* Schema.decodeUnknownEffect(HostedProjectCloneRequest, {
						onExcessProperty: "error",
					})({
						destination_path: input.destination_path,
						repository: input.repository,
						selection: input.selection,
					}).pipe(
						Effect.mapError((cause) =>
							client_error(
								"malformed",
								"The hosted project clone request is invalid.",
								cause,
							),
						),
					);
					const trace = yield* connection.MakeTrace;
					const envelope: HostedProjectCloneRequestEnvelope = {
						...trace,
						kind: "hosted.project.clone.request",
						message_id: input.command_id ?? trace.message_id,
						payload,
						thread_id: input.thread_id,
					};

					return yield* send_workspace_mutation(envelope);
				});
			const respond_hosted_project_clone_approval = (
				input: ArtisanHostedProjectCloneApprovalResponseInput,
			) =>
				Effect.gen(function* () {
					const trace = yield* connection.MakeTrace;
					const envelope: HostedProjectCloneApprovalRespondEnvelope = {
						...trace,
						kind: "hosted.project.clone.approval.respond",
						message_id: input.command_id ?? trace.message_id,
						payload: { approval_id: input.approval_id, approved: input.approved },
						thread_id: input.thread_id,
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
				GetHostedProjectCloneApproval: get_hosted_project_clone_approval,
				GetExternalWaits: get_external_waits,
				GetHostedGitSnapshot: get_hosted_git_snapshot,
				GetGlobalGuidance: get_global_guidance,
				GetModelBehaviour: get_model_behaviour,
				GetThreadRetentionPolicy: get_thread_retention_policy,
				GetThreadWork: get_thread_work,
				GetWorkspaceChangeDiff: get_workspace_change_diff,
				GetWorkspaceReplaceApproval: get_workspace_replace_approval,
				GetWorkspaceGitSession: get_workspace_git_session,
				GetWorkspaceGitCheckoutApproval: get_workspace_git_checkout_approval,
				GetWorkspaceGitMutationApproval: get_workspace_git_mutation_approval,
				ListWorkspaceChanges: list_workspace_changes,
				ListTerminals: list_terminals,
				ListThreads: list_threads,
				OpenAsset: (asset_id) => streams.Open(`asset:${asset_id}`),
				OpenTerminalOutput: (terminal_id) => streams.Open(`terminal:${terminal_id}`),
				ReadWorkspaceFile: read_workspace_file,
				ResolveGlobalGuidanceDrift: resolve_global_guidance_drift,
				ResolveModelBehaviourDrift: resolve_model_behaviour_drift,
				RetryGlobalGuidanceSync: retry_global_guidance_sync,
				RetryModelBehaviourSync: retry_model_behaviour_sync,
				ReplaceWorkspaceFile: replace_workspace_file,
				RespondWorkspaceReplaceApproval: respond_workspace_replace_approval,
				RefreshWorkspaceGitSession: refresh_workspace_git_session,
				RefreshHostedGitSnapshot: refresh_hosted_git_snapshot,
				RequestWorkspaceGitCheckout: request_workspace_git_checkout,
				RequestWorkspaceGitMutation: request_workspace_git_mutation,
				RequestHostedProjectClone: request_hosted_project_clone,
				RespondWorkspaceGitCheckoutApproval: respond_workspace_git_checkout_approval,
				RespondWorkspaceGitMutationApproval: respond_workspace_git_mutation_approval,
				RespondHostedProjectCloneApproval: respond_hosted_project_clone_approval,
				RequestExternalWait: request_external_wait,
				CancelExternalWait: cancel_external_wait,
				ManuallyResumeExternalWait: manually_resume_external_wait,
				ReviewWorkspaceChange: review_workspace_change,
				RollbackWorkspaceChange: rollback_workspace_change,
				SelectGlobalGuidance: select_global_guidance,
				SubscribeOrchestrationGraph: subscriptions.SubscribeOrchestrationGraph,
				SubscribeThreadList: subscriptions.SubscribeThreadList,
				UpdateGlobalGuidance: update_global_guidance,
				UpdateModelBehaviour: update_model_behaviour,
				UpdateThreadRetentionPolicy: update_thread_retention_policy,
			};
		}),
	);
}
