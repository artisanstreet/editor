import {
	Cause,
	Clock,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	Option,
	PubSub,
	Queue,
	Ref,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	SupportedProtocolVersions,
	type AckEnvelope,
	type CommandEnvelope,
	type EventEnvelope,
	type GlobalGuidanceDriftResolutionEnvelope,
	type GlobalGuidanceQueryEnvelope,
	type GlobalGuidanceRetryEnvelope,
	type GlobalGuidanceSelectionEnvelope,
	type GlobalGuidanceUpdateEnvelope,
	type GitDiffQueryEnvelope,
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
	type GitWorkspaceQueryEnvelope,
	type HeartbeatPongEnvelope,
	type HelloEnvelope,
	type InboundControlEnvelope,
	type ModelBehaviourDriftResolutionEnvelope,
	type ModelBehaviourQueryEnvelope,
	type ModelBehaviourRetryEnvelope,
	type ModelBehaviourUpdateEnvelope,
	type OrchestrationGraphQueryEnvelope,
	type OutboundControlEnvelope,
	type PreNegotiationProtocolErrorEnvelope,
	type ProtocolErrorDetail,
	type ProtocolErrorEnvelope,
	type ReplayEnvelope,
	type StreamCursor,
	type SubscribeEnvelope,
	type ThreadListItem,
	type ThreadListQueryEnvelope,
	type ThreadRetentionQueryEnvelope,
	type ThreadRetentionUpdateEnvelope,
	type ThreadWorkQueryEnvelope,
	type TerminalListQueryEnvelope,
	type UnsubscribeEnvelope,
	type WorkspaceChangeListQueryEnvelope,
	type WorkspaceChangeDiffQueryEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceFileReadQueryEnvelope,
	type WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";

import { GitService, GitServiceError } from "../git/git-service";
import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { GuidanceFileStoreFailure } from "../guidance/file-store";
import { global_guidance_thread_id } from "../guidance/guidance-repository";
import {
	GlobalGuidanceConflict,
	GlobalGuidanceInvariantError,
	GlobalGuidanceService,
} from "../guidance/guidance-service";
import { model_behaviour_thread_id } from "../model-behaviour/model-behaviour-repository";
import {
	ModelBehaviourConflict,
	ModelBehaviourInvariantError,
	ModelBehaviourService,
} from "../model-behaviour/model-behaviour-service";
import { JournalNotifier } from "../persistence/journal-notifier";
import {
	CommandIdConflict,
	JournalInvariantError,
	JournalStore,
	JournalStoreFailure,
} from "../persistence/journal-store";
import { OrchestrationRepository } from "../persistence/orchestration-repository";
import { ThreadReadModel } from "../persistence/thread-read-model";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { TerminalSessionService } from "../terminal/terminal-sessions";
import { thread_activity_kind_from_event } from "../threads/internal/thread-activity";
import {
	thread_retention_policy_thread_id,
	ThreadRetentionPolicyService,
} from "../threads/thread-retention-policy";
import { WorkspaceChangeRepository } from "../workspace/workspace-change-repository";
import {
	WorkspaceChangeDiffService,
	WorkspaceChangeDiffUnavailable,
} from "../workspace/workspace-change-diff-service";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
} from "../workspace/workspace-file-service";
import {
	DecodeProtocolConnectionOptions,
	DefaultProtocolConnectionOptions,
	type ProtocolConnection,
	type ProtocolConnectionOptions,
} from "./protocol-connection";
import { ProtocolRouter, type ProtocolRouterInboundDispatch } from "./protocol-router";

interface PendingHeartbeat {
	readonly deadline_ms: number;
	readonly message_id: string;
	readonly nonce: string;
}

interface ThreadListProjectionSubscription {
	readonly _tag: "thread.list";
	readonly sequence: number;
	readonly stream_id: string;
}

interface OrchestrationGraphProjectionSubscription {
	readonly _tag: "orchestration.graph";
	readonly group_id: string;
	readonly sequence: number;
	readonly stream_id: string;
}

type ProjectionSubscription =
	| OrchestrationGraphProjectionSubscription
	| ThreadListProjectionSubscription;

interface AwaitingHelloState {
	readonly _tag: "AwaitingHello";
	readonly last_activity_ms: number;
}

interface ReadyState {
	readonly _tag: "Ready";
	readonly acknowledged_cursors: Readonly<Record<string, number>>;
	readonly acknowledged_journal_sequence: number;
	readonly connection_id: string;
	readonly delivered_cursors: Readonly<Record<string, number>>;
	readonly delivered_journal_sequence: number;
	readonly last_activity_ms: number;
	readonly pending_heartbeat?: PendingHeartbeat;
	readonly stream_ticket: string;
	readonly subscriptions: Readonly<Record<string, ProjectionSubscription>>;
}

interface RejectedState {
	readonly _tag: "Rejected";
	readonly last_activity_ms: number;
}

interface ClosedState {
	readonly _tag: "Closed";
}

type ConnectionState = AwaitingHelloState | ReadyState | RejectedState | ClosedState;

function cursors_to_record(cursors: ReadonlyArray<StreamCursor>) {
	return Object.fromEntries(cursors.map((cursor) => [cursor.stream_id, cursor.sequence]));
}

function record_to_cursors(cursors: Readonly<Record<string, number>>) {
	return Object.entries(cursors)
		.map(([stream_id, sequence]) => ({ sequence, stream_id }))
		.sort((left, right) => left.stream_id.localeCompare(right.stream_id));
}

function apply_event_cursors(
	cursors: Readonly<Record<string, number>>,
	events: ReadonlyArray<EventEnvelope>,
) {
	return events.reduce<Readonly<Record<string, number>>>(
		(current, event) => ({
			...current,
			[event.stream_id]: Math.max(current[event.stream_id] ?? 0, event.sequence),
		}),
		cursors,
	);
}

function latest_journal_sequence(fallback: number, events: ReadonlyArray<EventEnvelope>) {
	return events.reduce((sequence, event) => Math.max(sequence, event.journal_sequence), fallback);
}

function guidance_error_detail(error: unknown): ProtocolErrorDetail {
	if (error instanceof CommandIdConflict) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	}

	if (error instanceof GlobalGuidanceConflict) {
		return {
			code: "guidance.conflict",
			message: "The provider guidance changed; refresh before retrying.",
			retryable: false,
		};
	}

	if (error instanceof JournalInvariantError) {
		return {
			code: "journal.invariant_failed",
			message: "The journal could not reconstruct the guidance command.",
			retryable: false,
		};
	}

	if (error instanceof GlobalGuidanceInvariantError) {
		return {
			code: "guidance.invariant_failed",
			message: "The canonical guidance state failed validation.",
			retryable: false,
		};
	}

	if (error instanceof GuidanceFileStoreFailure || error instanceof JournalStoreFailure) {
		return {
			code: "guidance.unavailable",
			message: "Global guidance could not be durably updated.",
			retryable: true,
		};
	}

	return {
		code: "guidance.unavailable",
		message: "Global guidance could not be durably updated.",
		retryable: true,
	};
}

function model_behaviour_error_detail(error: unknown): ProtocolErrorDetail {
	if (error instanceof CommandIdConflict) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	}

	if (error instanceof ModelBehaviourConflict) {
		return {
			code: "model_behaviour.conflict",
			message: error.message,
			retryable: false,
		};
	}

	if (error instanceof JournalInvariantError || error instanceof ModelBehaviourInvariantError) {
		return {
			code: "model_behaviour.invariant_failed",
			message: "The canonical Model Behaviour state failed validation.",
			retryable: false,
		};
	}

	return {
		code: "model_behaviour.unavailable",
		message: "Model Behaviour settings could not be durably reconciled.",
		retryable: true,
	};
}

function workspace_error_detail(error: unknown): ProtocolErrorDetail {
	if (error instanceof WorkspaceFileServiceError && error.reason === "changed") {
		return {
			code: "workspace.conflict",
			message: "The workspace file changed before the requested mutation could apply.",
			retryable: false,
		};
	}

	return {
		code: "workspace.unavailable",
		message: "The workspace operation could not be completed.",
		retryable: true,
	};
}

function workspace_diff_error_detail(error: unknown): ProtocolErrorDetail {
	if (
		error instanceof WorkspaceChangeDiffUnavailable &&
		(error.reason === "legacy_unavailable" || error.reason === "missing")
	) {
		return {
			code: "workspace.diff_unavailable",
			message: "No immutable diff is available for this workspace change.",
			retryable: false,
		};
	}

	if (error instanceof WorkspaceChangeDiffUnavailable && error.reason === "erased") {
		return {
			code: "workspace.unavailable",
			message: "The workspace change is no longer available.",
			retryable: false,
		};
	}

	return {
		code: "workspace.invariant_failed",
		message: "The immutable workspace diff failed validation.",
		retryable: false,
	};
}

function git_error_detail(error: unknown): ProtocolErrorDetail {
	if (error instanceof CommandIdConflict) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different intent.",
			retryable: false,
		};
	}

	if (error instanceof GitServiceError) {
		switch (error.reason) {
			case "busy":
				return {
					code: "git.busy",
					message: "Another Git mutation is already active for this workspace.",
					retryable: true,
				};
			case "changed":
				return {
					code: "git.changed",
					message: "The Git workspace changed; refresh before retrying.",
					retryable: false,
				};
			case "id_conflict":
				return {
					code: "command.id_conflict",
					message: "This command id has already been used for different Git intent.",
					retryable: false,
				};
			case "invalid_path":
				return {
					code: "git.invalid_path",
					message: "One or more paths are not eligible for this Git mutation.",
					retryable: false,
				};
			case "invariant":
				return {
					code: "git.invariant_failed",
					message: "The durable Git state failed validation.",
					retryable: false,
				};
			case "not_repository":
				return {
					code: "git.not_repository",
					message: "The workspace is not a Git repository.",
					retryable: false,
				};
			case "unsupported_state":
				return {
					code: "git.unsupported_state",
					message: "This Git mutation is not supported in the current repository state.",
					retryable: false,
				};
			case "unavailable":
				return {
					code: "git.unavailable",
					message: "The Git operation could not be completed.",
					retryable: error.retryable,
				};
		}
	}

	const tagged =
		typeof error === "object" && error !== null
			? (error as { readonly _tag?: unknown; readonly reason?: unknown })
			: undefined;
	const tag = typeof tagged?._tag === "string" ? tagged._tag : "";
	const reason = typeof tagged?.reason === "string" ? tagged.reason : "";

	if (reason === "changed" || reason === "snapshot_changed" || reason === "workspace_changed") {
		return {
			code: "git.changed",
			message: "The Git workspace changed; refresh before retrying.",
			retryable: false,
		};
	}

	if (
		reason === "decision_conflict" ||
		reason === "id_conflict" ||
		reason === "mutation_conflict"
	) {
		return {
			code: "command.id_conflict",
			message: "This command id has already been used for different Git intent.",
			retryable: false,
		};
	}

	if (reason === "busy" || reason === "workspace_busy") {
		return {
			code: "git.busy",
			message: "Another Git mutation is already active for this workspace.",
			retryable: true,
		};
	}

	if (reason === "not_repository") {
		return {
			code: "git.not_repository",
			message: "The workspace is not a Git repository.",
			retryable: false,
		};
	}

	if (
		reason === "not_found" ||
		reason === "thread_unavailable" ||
		reason === "unauthorized" ||
		reason === "workspace_unavailable"
	) {
		return {
			code: "git.unavailable",
			message: "The Git workspace is not available.",
			retryable: false,
		};
	}

	if (
		tag.includes("Invariant") ||
		tag.includes("Invalid") ||
		reason === "corrupt" ||
		reason === "invariant"
	) {
		return {
			code: "git.invariant_failed",
			message: "The durable Git state failed validation.",
			retryable: false,
		};
	}

	return {
		code: "git.unavailable",
		message: "The Git operation could not be completed.",
		retryable: true,
	};
}

function thread_item_from_event(event: EventEnvelope): ThreadListItem | undefined {
	if (event.payload.type === "thread.metadata.updated") {
		return event.payload.thread;
	}
	if (event.payload.type === "thread.project_affinity.updated") {
		return event.payload.thread;
	}

	return event.payload.type === "thread.created"
		? {
				activity_version: 0,
				affinity_version: 0,
				created_at: event.sent_at,
				current_goal: event.payload.title,
				last_activity_at: event.sent_at,
				live_status: "Idle",
				metadata_version: 0,
				pinned: false,
				linked_projects: [],
				project_affinity_scores: [],
				project_locked: false,
				thread_id: event.thread_id,
				title: event.payload.title,
				title_locked: false,
				title_source: "initial",
				updated_at: event.sent_at,
			}
		: undefined;
}

type ThreadListProjectionPatch =
	| { readonly _tag: "Remove"; readonly thread_id: string }
	| { readonly _tag: "Upsert"; readonly thread: ThreadListItem };

function direct_thread_list_patch_from_event(
	event: EventEnvelope,
): ThreadListProjectionPatch | undefined {
	if (event.payload.type === "thread.erased") {
		return { _tag: "Remove", thread_id: event.thread_id };
	}

	const thread = thread_item_from_event(event);

	return thread ? { _tag: "Upsert", thread } : undefined;
}

function graph_group_id_from_event(event: EventEnvelope) {
	return event.payload.type === "orchestration.graph.lifecycle" ||
		event.payload.type === "assignment.heartbeat" ||
		event.payload.type === "agent_instance.renamed" ||
		event.payload.type === "assignment.control" ||
		event.payload.type === "artifact.recorded"
		? event.payload.group_id
		: undefined;
}

/** Owns scoped, transport-neutral Artisan control connections. */
export class ProtocolServer extends Context.Service<
	ProtocolServer,
	{
		readonly Open: Effect.Effect<ProtocolConnection, never, Scope.Scope>;
	}
>()("Artisan/ProtocolServer") {}

export function make_protocol_server_layer(
	input_options: ProtocolConnectionOptions = DefaultProtocolConnectionOptions,
) {
	return Layer.effect(
		ProtocolServer,
		Effect.gen(function* () {
			const options = yield* DecodeProtocolConnectionOptions(input_options);
			const git = yield* GitService;
			const graph = yield* AgentGraphOrchestrator;
			const guidance = yield* GlobalGuidanceService;
			const model_behaviour = yield* ModelBehaviourService;
			const journal = yield* JournalStore;
			const metadata = yield* RuntimeMetadata;
			const notifier = yield* JournalNotifier;
			const router = yield* ProtocolRouter;
			const orchestration = yield* OrchestrationRepository;
			const terminals = yield* TerminalSessionService;
			const thread_read_model = yield* ThreadReadModel;
			const retention_policy = yield* ThreadRetentionPolicyService;
			const workspace_changes = yield* WorkspaceChangeRepository;
			const workspace_diffs = yield* WorkspaceChangeDiffService;
			const workspace_files = yield* WorkspaceFileService;

			const Open = Effect.gen(function* () {
				const connection_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
					Scope.close(scope, Exit.succeed(undefined)),
				);
				const initial_time = yield* Clock.currentTimeMillis;
				const outbound = yield* Effect.acquireRelease(
					Queue.bounded<OutboundControlEnvelope>(options.outbound_capacity),
					Queue.shutdown,
				).pipe(Scope.provide(connection_scope));
				const journal_subscription = yield* notifier.Subscribe.pipe(
					Scope.provide(connection_scope),
				);
				const state = yield* Ref.make<ConnectionState>({
					_tag: "AwaitingHello",
					last_activity_ms: initial_time,
				});
				const closed = yield* Deferred.make<void>();
				const connection_ready = yield* Deferred.make<void>();
				const receive_lock = yield* Semaphore.make(1);

				const Enqueue = (envelope: OutboundControlEnvelope) =>
					Queue.offer(outbound, envelope).pipe(Effect.asVoid);

				yield* Scope.addFinalizer(
					connection_scope,
					Semaphore.withPermit(receive_lock)(
						Effect.gen(function* () {
							yield* Ref.set(state, { _tag: "Closed" });
							yield* Deferred.succeed(connection_ready, undefined);
							yield* Deferred.succeed(closed, undefined);
						}),
					),
				);

				const BeginClose = Semaphore.withPermit(receive_lock)(
					Effect.gen(function* () {
						const current = yield* Ref.get(state);

						if (current._tag === "Closed") {
							return false;
						}

						yield* Ref.set(state, { _tag: "Closed" });
						yield* Deferred.succeed(connection_ready, undefined);

						return true;
					}),
				);

				const RequestClose = BeginClose.pipe(
					Effect.flatMap((should_close) =>
						should_close
							? Scope.close(connection_scope, Exit.succeed(undefined)).pipe(
									Effect.forkDetach,
									Effect.asVoid,
								)
							: Effect.void,
					),
				);
				const Close = RequestClose.pipe(Effect.andThen(Deferred.await(closed)));

				const MakeError = (
					current: ConnectionState,
					detail: ProtocolErrorDetail,
					correlation_id?: string,
				) =>
					Effect.gen(function* () {
						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;

						if (current._tag !== "Ready") {
							const error: PreNegotiationProtocolErrorEnvelope = {
								kind: "protocol.error",
								message_id,
								origin: "backend",
								payload: detail,
								schema_version: 1,
								sent_at,
								...(correlation_id ? { correlation_id } : {}),
							};

							return error;
						}

						const error: ProtocolErrorEnvelope = {
							kind: "protocol.error",
							message_id,
							origin: "backend",
							payload: detail,
							protocol_version: 1,
							schema_version: 1,
							sent_at,
							...(correlation_id ? { correlation_id } : {}),
						};

						return error;
					});

				const EnqueueError = (
					current: ConnectionState,
					code: string,
					message: string,
					retryable: boolean,
					correlation_id?: string,
				) =>
					MakeError(current, { code, message, retryable }, correlation_id).pipe(
						Effect.flatMap(Enqueue),
					);

				const EnqueueProjectionPatches = (current: ReadyState, event: EventEnvelope) =>
					Effect.gen(function* () {
						let subscriptions = current.subscriptions;
						const has_thread_list = Object.values(current.subscriptions).some(
							(subscription) => subscription._tag === "thread.list",
						);
						let thread_patch = direct_thread_list_patch_from_event(event);

						if (
							has_thread_list &&
							!thread_patch &&
							thread_activity_kind_from_event(event.payload) !== undefined
						) {
							const thread = yield* thread_read_model.Lookup(event.thread_id);

							thread_patch = Option.match(thread, {
								onNone: () => undefined,
								onSome: (item) => ({ _tag: "Upsert" as const, thread: item }),
							});
						}

						for (const [subscription_id, subscription] of Object.entries(
							current.subscriptions,
						)) {
							const message_id = yield* metadata.MakeId("message");
							const sequence = subscription.sequence + 1;

							if (subscription._tag === "thread.list") {
								if (!thread_patch) {
									continue;
								}

								if (thread_patch._tag === "Remove") {
									yield* Enqueue({
										journal_sequence: event.journal_sequence,
										kind: "thread.list.remove",
										message_id,
										origin: "backend",
										payload: { thread_id: thread_patch.thread_id },
										protocol_version: 1,
										schema_version: 1,
										sent_at: event.sent_at,
										sequence,
										stream_id: subscription.stream_id,
										subscription_id,
									});
								} else {
									yield* Enqueue({
										journal_sequence: event.journal_sequence,
										kind: "thread.list.upsert",
										message_id,
										origin: "backend",
										payload: thread_patch.thread,
										protocol_version: 1,
										schema_version: 1,
										sent_at: event.sent_at,
										sequence,
										stream_id: subscription.stream_id,
										subscription_id,
									});
								}
							} else {
								const group_id = graph_group_id_from_event(event);

								if (group_id !== subscription.group_id) {
									continue;
								}

								const projection = yield* graph.GetGraph(group_id);

								yield* Enqueue({
									journal_sequence: projection.journal_sequence,
									kind: "orchestration.graph.patch",
									message_id,
									origin: "backend",
									payload: { graph: projection },
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							}

							subscriptions = {
								...subscriptions,
								[subscription_id]: { ...subscription, sequence },
							};
						}

						return subscriptions;
					});

				const DeliverLiveEvents = (events: ReadonlyArray<EventEnvelope>) =>
					Effect.gen(function* () {
						const current = yield* Ref.get(state);

						if (current._tag !== "Ready") {
							return;
						}

						let subscriptions = current.subscriptions;
						const new_events = events.filter(
							(event) => event.journal_sequence > current.delivered_journal_sequence,
						);

						for (const event of new_events) {
							yield* Enqueue(event);
							subscriptions = yield* EnqueueProjectionPatches(
								{ ...current, subscriptions },
								event,
							);
						}

						yield* Ref.set(state, {
							...current,
							delivered_cursors: apply_event_cursors(
								current.delivered_cursors,
								new_events,
							),
							delivered_journal_sequence: latest_journal_sequence(
								current.delivered_journal_sequence,
								new_events,
							),
							subscriptions,
						});
					});

				const EnqueueReplayEvents = (events: ReadonlyArray<EventEnvelope>) =>
					Effect.gen(function* () {
						const current = yield* Ref.get(state);

						if (current._tag !== "Ready") {
							return;
						}

						yield* Effect.forEach(events, Enqueue, { discard: true });
						yield* Ref.set(state, {
							...current,
							delivered_cursors: apply_event_cursors(
								current.delivered_cursors,
								events,
							),
							delivered_journal_sequence: latest_journal_sequence(
								current.delivered_journal_sequence,
								events,
							),
						});
					});

				const HandleHello = (hello: HelloEnvelope, current: AwaitingHelloState) =>
					Effect.gen(function* () {
						const supports_version = hello.payload.supported_protocol_versions.includes(
							SupportedProtocolVersions[0],
						);

						if (!supports_version) {
							yield* EnqueueError(
								current,
								"protocol.unsupported_version",
								"No supported protocol version was offered.",
								false,
								hello.message_id,
							);
							yield* Ref.set(state, {
								_tag: "Rejected",
								last_activity_ms: current.last_activity_ms,
							});

							return;
						}

						return yield* journal
							.ReadReplay({
								after_journal_sequence: hello.payload.last_journal_sequence,
								stream_cursors: hello.payload.event_cursors,
							})
							.pipe(
								Effect.flatMap((events) =>
									Effect.gen(function* () {
										const connection_id = yield* metadata.MakeId("connection");
										const stream_ticket =
											yield* metadata.MakeId("stream_ticket");
										const welcome_id = yield* metadata.MakeId("message");
										const replay_id = yield* metadata.MakeId("message");
										const sent_at = yield* metadata.Now;
										const delivered_cursors = apply_event_cursors(
											cursors_to_record(hello.payload.event_cursors),
											events,
										);
										const journal_sequence = latest_journal_sequence(
											hello.payload.last_journal_sequence,
											events,
										);
										const ready: ReadyState = {
											_tag: "Ready",
											acknowledged_cursors: cursors_to_record(
												hello.payload.event_cursors,
											),
											acknowledged_journal_sequence:
												hello.payload.last_journal_sequence,
											connection_id,
											delivered_cursors,
											delivered_journal_sequence: journal_sequence,
											last_activity_ms: current.last_activity_ms,
											stream_ticket,
											subscriptions: {},
										};

										yield* Enqueue({
											correlation_id: hello.message_id,
											kind: "welcome",
											message_id: welcome_id,
											origin: "backend",
											payload: {
												connection_id,
												current_event_cursors:
													record_to_cursors(delivered_cursors),
												heartbeat_interval_ms:
													options.heartbeat_interval_ms,
												heartbeat_timeout_ms: options.heartbeat_timeout_ms,
												journal_sequence,
												stream_ticket,
											},
											protocol_version: 1,
											schema_version: 1,
											sent_at,
										});
										yield* Ref.set(state, ready);
										yield* Effect.forEach(events, Enqueue, { discard: true });
										yield* Enqueue({
											correlation_id: hello.message_id,
											kind: "replay.complete",
											message_id: replay_id,
											origin: "backend",
											payload: {
												current_event_cursors:
													record_to_cursors(delivered_cursors),
												journal_sequence,
											},
											protocol_version: 1,
											schema_version: 1,
											sent_at,
										});
										yield* Deferred.succeed(connection_ready, undefined);
									}),
								),
								Effect.catch(() =>
									EnqueueError(
										current,
										"protocol.resume_invalid",
										"The supplied resume cursor does not match the journal.",
										false,
										hello.message_id,
									),
								),
							);
					});

				const HandleQuery = (query: ThreadListQueryEnvelope, current: ReadyState) =>
					thread_read_model.Snapshot().pipe(
						Effect.flatMap((snapshot) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "thread.list.query.result",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"projection.unavailable",
								"The thread projection could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleRetentionQuery = (
					query: ThreadRetentionQueryEnvelope,
					current: ReadyState,
				) =>
					retention_policy.Read.pipe(
						Effect.flatMap((policy) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "thread.retention.query.result",
									message_id,
									origin: "backend",
									payload: policy,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"retention.unavailable",
								"The thread retention policy could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleGuidanceQuery = (
					query: GlobalGuidanceQueryEnvelope,
					current: ReadyState,
				) =>
					guidance.Get.pipe(
						Effect.flatMap((snapshot) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "guidance.query.result",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"guidance.unavailable",
								"Global guidance could not be read or reconciled.",
								true,
								query.message_id,
							),
						),
					);

				const HandleModelBehaviourQuery = (
					query: ModelBehaviourQueryEnvelope,
					current: ReadyState,
				) =>
					model_behaviour.Get.pipe(
						Effect.flatMap((snapshot) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "model_behaviour.query.result",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"model_behaviour.unavailable",
								"Model Behaviour settings could not be read or reconciled.",
								true,
								query.message_id,
							),
						),
					);

				const HandleWorkQuery = (query: ThreadWorkQueryEnvelope, current: ReadyState) =>
					orchestration.GetWork(query.payload.thread_id).pipe(
						Effect.flatMap((work) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "thread.work.query.result",
									message_id,
									origin: "backend",
									payload: work ? { work } : {},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"projection.unavailable",
								"The thread work projection could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleTerminalListQuery = (
					query: TerminalListQueryEnvelope,
					current: ReadyState,
				) =>
					terminals.List(query.payload.thread_id, query.payload.workspace_id).pipe(
						Effect.flatMap((terminals) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "terminal.list.query.result",
									message_id,
									origin: "backend",
									payload: { terminals },
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"projection.unavailable",
								"The terminal projection could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleGraphQuery = (
					query: OrchestrationGraphQueryEnvelope,
					current: ReadyState,
				) =>
					graph.GetGraph(query.payload.group_id).pipe(
						Effect.flatMap((projection) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "orchestration.graph.query.result",
									message_id,
									origin: "backend",
									payload: { graph: projection },
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"projection.unavailable",
								"The orchestration graph projection could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleWorkspaceFileReadQuery = (
					query: WorkspaceFileReadQueryEnvelope,
					current: ReadyState,
				) =>
					workspace_files.Read(query.payload).pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "workspace.file.read.query.result",
									message_id,
									origin: "backend",
									payload: result,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch((error) => {
							const detail = workspace_error_detail(error);

							return EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								query.message_id,
							);
						}),
					);

				const HandleWorkspaceChangeListQuery = (
					query: WorkspaceChangeListQueryEnvelope,
					current: ReadyState,
				) =>
					workspace_changes
						.List(query.payload.thread_id, query.payload.workspace_id)
						.pipe(
							Effect.flatMap((result) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* Enqueue({
										correlation_id: query.message_id,
										kind: "workspace.change.list.query.result",
										message_id,
										origin: "backend",
										payload: result,
										protocol_version: 1,
										schema_version: 1,
										sent_at,
									});
								}),
							),
							Effect.catch(() =>
								EnqueueError(
									current,
									"projection.unavailable",
									"The workspace change projection could not be read.",
									true,
									query.message_id,
								),
							),
						);

				const HandleWorkspaceChangeDiffQuery = (
					query: WorkspaceChangeDiffQueryEnvelope,
					current: ReadyState,
				) =>
					workspace_diffs.Read(query.payload).pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "workspace.change.diff.query.result",
									message_id,
									origin: "backend",
									payload: result,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch((error) => {
							const detail = workspace_diff_error_detail(error);

							return EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								query.message_id,
							);
						}),
					);

				const HandleGitWorkspaceQuery = (
					query: GitWorkspaceQueryEnvelope,
					current: ReadyState,
				) =>
					git.Query(query).pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "git.workspace.query.result",
									message_id,
									origin: "backend",
									payload: result,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch((error) => {
							const detail = git_error_detail(error);

							return EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								query.message_id,
							);
						}),
					);

				const HandleGitDiffQuery = (query: GitDiffQueryEnvelope, current: ReadyState) =>
					git.Diff(query).pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "git.diff.query.result",
									message_id,
									origin: "backend",
									payload: result,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catch((error) => {
							const detail = git_error_detail(error);

							return EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								query.message_id,
							);
						}),
					);

				const HandleSubscribe = (subscribe: SubscribeEnvelope, current: ReadyState) =>
					Effect.gen(function* () {
						if (current.subscriptions[subscribe.subscription_id]) {
							yield* EnqueueError(
								current,
								"subscription.already_exists",
								"The subscription id is already active.",
								false,
								subscribe.message_id,
							);

							return;
						}

						if (subscribe.payload.type === "orchestration.graph") {
							const group_id = subscribe.payload.group_id;

							return yield* graph.GetGraph(group_id).pipe(
								Effect.flatMap((projection) =>
									Effect.gen(function* () {
										const started_id = yield* metadata.MakeId("message");
										const snapshot_id = yield* metadata.MakeId("message");
										const sent_at = yield* metadata.Now;
										const stream_id = `projection:orchestration.graph:${group_id}:${subscribe.subscription_id}`;
										const subscription: OrchestrationGraphProjectionSubscription =
											{
												_tag: "orchestration.graph",
												group_id,
												sequence: 0,
												stream_id,
											};

										yield* Ref.set(state, {
											...current,
											subscriptions: {
												...current.subscriptions,
												[subscribe.subscription_id]: subscription,
											},
										});
										yield* Enqueue({
											correlation_id: subscribe.message_id,
											kind: "subscription.started",
											message_id: started_id,
											origin: "backend",
											payload: { stream_id },
											protocol_version: 1,
											schema_version: 1,
											sent_at,
											subscription_id: subscribe.subscription_id,
										});
										yield* Enqueue({
											journal_sequence: projection.journal_sequence,
											kind: "orchestration.graph.snapshot",
											message_id: snapshot_id,
											origin: "backend",
											payload: { graph: projection },
											protocol_version: 1,
											schema_version: 1,
											sent_at,
											sequence: 0,
											stream_id,
											subscription_id: subscribe.subscription_id,
										});
									}),
								),
								Effect.catch(() =>
									EnqueueError(
										current,
										"projection.unavailable",
										"The orchestration graph projection could not be read.",
										true,
										subscribe.message_id,
									),
								),
							);
						}

						return yield* thread_read_model.Snapshot().pipe(
							Effect.flatMap((snapshot) =>
								Effect.gen(function* () {
									const started_id = yield* metadata.MakeId("message");
									const snapshot_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;
									const stream_id = `projection:thread.list:${subscribe.subscription_id}`;
									const subscription: ThreadListProjectionSubscription = {
										_tag: "thread.list",
										sequence: 0,
										stream_id,
									};

									yield* Ref.set(state, {
										...current,
										subscriptions: {
											...current.subscriptions,
											[subscribe.subscription_id]: subscription,
										},
									});
									yield* Enqueue({
										correlation_id: subscribe.message_id,
										kind: "subscription.started",
										message_id: started_id,
										origin: "backend",
										payload: { stream_id },
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										subscription_id: subscribe.subscription_id,
									});
									yield* Enqueue({
										journal_sequence: snapshot.journal_sequence,
										kind: "thread.list.snapshot",
										message_id: snapshot_id,
										origin: "backend",
										payload: { threads: snapshot.threads },
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										sequence: 0,
										stream_id,
										subscription_id: subscribe.subscription_id,
									});
								}),
							),
							Effect.catch(() =>
								EnqueueError(
									current,
									"projection.unavailable",
									"The thread projection could not be read.",
									true,
									subscribe.message_id,
								),
							),
						);
					});

				const HandleUnsubscribe = (unsubscribe: UnsubscribeEnvelope, current: ReadyState) =>
					Effect.gen(function* () {
						if (!current.subscriptions[unsubscribe.subscription_id]) {
							yield* EnqueueError(
								current,
								"subscription.not_found",
								"The subscription id is not active.",
								false,
								unsubscribe.message_id,
							);

							return;
						}

						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;
						const subscriptions = { ...current.subscriptions };

						delete subscriptions[unsubscribe.subscription_id];
						yield* Ref.set(state, { ...current, subscriptions });
						yield* Enqueue({
							correlation_id: unsubscribe.message_id,
							kind: "subscription.stopped",
							message_id,
							origin: "backend",
							payload: {},
							protocol_version: 1,
							schema_version: 1,
							sent_at,
							subscription_id: unsubscribe.subscription_id,
						});
					});

				const HandleAck = (ack: AckEnvelope, current: ReadyState) =>
					Effect.gen(function* () {
						const invalid_journal =
							ack.payload.journal_sequence < current.acknowledged_journal_sequence ||
							ack.payload.journal_sequence > current.delivered_journal_sequence;
						const invalid_stream = ack.payload.event_cursors.some((cursor) => {
							const acknowledged =
								current.acknowledged_cursors[cursor.stream_id] ?? 0;
							const delivered = current.delivered_cursors[cursor.stream_id] ?? 0;

							return cursor.sequence < acknowledged || cursor.sequence > delivered;
						});

						if (invalid_journal || invalid_stream) {
							yield* EnqueueError(
								current,
								"protocol.invalid_ack",
								"The acknowledgement is outside the delivered range.",
								false,
								ack.message_id,
							);

							return;
						}

						const valid_replay_point = yield* journal
							.ValidateReplayPoint({
								after_journal_sequence: ack.payload.journal_sequence,
								stream_cursors: ack.payload.event_cursors,
							})
							.pipe(
								Effect.as(true),
								Effect.catch(() => Effect.succeed(false)),
							);

						if (!valid_replay_point) {
							yield* EnqueueError(
								current,
								"protocol.invalid_ack",
								"The acknowledgement does not identify a durable replay point.",
								false,
								ack.message_id,
							);

							return;
						}

						yield* Ref.set(state, {
							...current,
							acknowledged_cursors: {
								...current.acknowledged_cursors,
								...cursors_to_record(ack.payload.event_cursors),
							},
							acknowledged_journal_sequence: ack.payload.journal_sequence,
						});
					});

				const HandleReplay = (replay: ReplayEnvelope, current: ReadyState) =>
					journal
						.ReadReplay({
							after_journal_sequence: replay.payload.after_journal_sequence,
							...(replay.payload.event_cursors
								? { stream_cursors: replay.payload.event_cursors }
								: {}),
						})
						.pipe(
							Effect.flatMap((events) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* EnqueueReplayEvents(events);
									const updated = yield* Ref.get(state);

									if (updated._tag !== "Ready") {
										return;
									}

									yield* Enqueue({
										correlation_id: replay.message_id,
										kind: "replay.complete",
										message_id,
										origin: "backend",
										payload: {
											current_event_cursors: record_to_cursors(
												updated.delivered_cursors,
											),
											journal_sequence: updated.delivered_journal_sequence,
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at,
									});
								}),
							),
							Effect.catch(() =>
								EnqueueError(
									current,
									"protocol.replay_invalid",
									"The replay cursor does not match the journal.",
									false,
									replay.message_id,
								),
							),
						);

				const HandlePong = (pong: HeartbeatPongEnvelope, current: ReadyState) =>
					Effect.gen(function* () {
						const pending = current.pending_heartbeat;
						const matches =
							pending?.message_id === pong.correlation_id &&
							pending.nonce === pong.payload.nonce;

						if (!matches) {
							yield* EnqueueError(
								current,
								"protocol.invalid_heartbeat",
								"The heartbeat response does not match an active ping.",
								false,
								pong.message_id,
							);

							return;
						}

						const { pending_heartbeat: _, ...without_pending } = current;

						yield* Ref.set(state, without_pending);
					});

				const HandleCommand = (command: CommandEnvelope) =>
					Effect.gen(function* () {
						const output = yield* router.RouteCommand(command);
						const events = output.filter(
							(envelope): envelope is EventEnvelope => envelope.kind === "event",
						);
						const non_events = output.filter((envelope) => envelope.kind !== "event");

						yield* Effect.forEach(non_events, Enqueue, { discard: true });

						const current = yield* Ref.get(state);
						const new_events =
							current._tag === "Ready"
								? events.filter(
										(event) =>
											event.journal_sequence >
											current.delivered_journal_sequence,
									)
								: [];

						if (new_events.length > 0) {
							yield* DeliverLiveEvents(new_events);
						}
					});
				const HandleRetentionUpdate = (update: ThreadRetentionUpdateEnvelope) =>
					HandleCommand({
						kind: "command",
						message_id: update.message_id,
						origin: update.origin,
						payload: {
							...update.payload,
							type: "thread.retention.update",
						},
						protocol_version: update.protocol_version,
						schema_version: update.schema_version,
						sent_at: update.sent_at,
						thread_id: thread_retention_policy_thread_id,
					});
				type GuidanceMutationEnvelope =
					| GlobalGuidanceDriftResolutionEnvelope
					| GlobalGuidanceRetryEnvelope
					| GlobalGuidanceSelectionEnvelope
					| GlobalGuidanceUpdateEnvelope;
				const HandleGuidanceMutation = (envelope: GuidanceMutationEnvelope) => {
					const trace = {
						message_id: envelope.message_id,
						origin: envelope.origin,
						sent_at: envelope.sent_at,
					};
					const operation =
						envelope.kind === "guidance.update"
							? guidance.Update({ ...trace, content: envelope.payload.content })
							: envelope.kind === "guidance.selection"
								? guidance.Select({ ...trace, ...envelope.payload })
								: envelope.kind === "guidance.drift.resolve"
									? guidance.ResolveDrift({ ...trace, ...envelope.payload })
									: guidance.RetrySync({ ...trace, ...envelope.payload });

					return operation.pipe(
						Effect.flatMap(({ acceptance }) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id,
									origin: "backend",
									payload: {
										journal_sequence: acceptance.event.journal_sequence,
										status: acceptance.status,
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									thread_id: global_guidance_thread_id,
								});

								const latest = yield* Ref.get(state);

								if (
									latest._tag === "Ready" &&
									acceptance.event.journal_sequence >
										latest.delivered_journal_sequence
								) {
									yield* DeliverLiveEvents([acceptance.event]);
								}
							}),
						),
						Effect.catch((error) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id,
									origin: "backend",
									payload: {
										error: guidance_error_detail(error),
										status: "rejected",
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									thread_id: global_guidance_thread_id,
								});
							}),
						),
					);
				};
				type ModelBehaviourMutationEnvelope =
					| ModelBehaviourDriftResolutionEnvelope
					| ModelBehaviourRetryEnvelope
					| ModelBehaviourUpdateEnvelope;
				const HandleModelBehaviourMutation = (envelope: ModelBehaviourMutationEnvelope) => {
					const trace = {
						message_id: envelope.message_id,
						origin: envelope.origin,
						sent_at: envelope.sent_at,
					};
					const operation =
						envelope.kind === "model_behaviour.update"
							? model_behaviour.Update({ ...trace, ...envelope.payload })
							: envelope.kind === "model_behaviour.drift.resolve"
								? model_behaviour.ResolveDrift({ ...trace, ...envelope.payload })
								: model_behaviour.RetrySync({ ...trace, ...envelope.payload });

					return operation.pipe(
						Effect.flatMap((result) =>
							Effect.gen(function* () {
								const events = yield* journal.ReadCorrelatedEvents(
									envelope.message_id,
								);
								const journal_sequence =
									events.at(-1)?.journal_sequence ??
									(yield* journal.ReadWatermark());
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id,
									origin: "backend",
									payload: {
										journal_sequence,
										status: result.status,
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									thread_id: model_behaviour_thread_id,
								});

								const latest = yield* Ref.get(state);
								const undelivered =
									latest._tag === "Ready"
										? events.filter(
												(event) =>
													event.journal_sequence >
													latest.delivered_journal_sequence,
											)
										: [];

								if (undelivered.length > 0) {
									yield* DeliverLiveEvents(undelivered);
								}
							}),
						),
						Effect.catch((error) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;

								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id,
									origin: "backend",
									payload: {
										error: model_behaviour_error_detail(error),
										status: "rejected",
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									thread_id: model_behaviour_thread_id,
								});
							}),
						),
					);
				};
				type WorkspaceMutationEnvelope =
					| WorkspaceChangeReviewEnvelope
					| WorkspaceChangeRollbackEnvelope
					| WorkspaceFileReplaceEnvelope;
				const HandleWorkspaceMutation = (envelope: WorkspaceMutationEnvelope) => {
					const operation =
						envelope.kind === "workspace.file.replace"
							? workspace_files.Replace({
									...envelope.payload,
									agent_id: envelope.agent_id,
									message_id: envelope.message_id,
									raw_origin: envelope.raw_origin,
									run_id: envelope.run_id,
									sent_at: envelope.sent_at,
									thread_id: envelope.thread_id,
								})
							: envelope.kind === "workspace.change.review"
								? workspace_files.Review({
										...envelope.payload,
										message_id: envelope.message_id,
										sent_at: envelope.sent_at,
										thread_id: envelope.thread_id,
									})
								: workspace_files.Rollback({
										...envelope.payload,
										message_id: envelope.message_id,
										sent_at: envelope.sent_at,
										thread_id: envelope.thread_id,
									});

					return operation.pipe(
						Effect.matchEffect({
							onFailure: (error) => {
								const detail = workspace_error_detail(error);

								return Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* Enqueue({
										causation_id: envelope.message_id,
										correlation_id: envelope.message_id,
										kind: "command.receipt",
										message_id,
										origin: "backend",
										payload: { error: detail, status: "rejected" },
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										thread_id: envelope.thread_id,
									});
								});
							},
							onSuccess: (acceptance) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* Enqueue({
										causation_id: envelope.message_id,
										correlation_id: envelope.message_id,
										kind: "command.receipt",
										message_id,
										origin: "backend",
										payload: {
											journal_sequence: acceptance.event.journal_sequence,
											status: acceptance.status,
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										thread_id: envelope.thread_id,
									});
								}),
						}),
					);
				};
				type GitMutationEnvelope =
					| GitIndexStageRequestEnvelope
					| GitIndexUnstageRequestEnvelope
					| GitMutationResolveEnvelope;
				const HandleGitMutation = (envelope: GitMutationEnvelope) => {
					const operation =
						envelope.kind === "git.mutation.resolve"
							? git.Resolve(envelope)
							: git.Request(envelope);

					return operation.pipe(
						Effect.matchEffect({
							onFailure: (error) => {
								const detail = git_error_detail(error);

								return Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* Enqueue({
										causation_id: envelope.message_id,
										correlation_id: envelope.message_id,
										kind: "command.receipt",
										message_id,
										origin: "backend",
										payload: { error: detail, status: "rejected" },
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										thread_id: envelope.thread_id,
										...(envelope.agent_id === undefined
											? {}
											: { agent_id: envelope.agent_id }),
										...(envelope.run_id === undefined
											? {}
											: { run_id: envelope.run_id }),
									});
								});
							},
							onSuccess: (acceptance) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;

									yield* Enqueue({
										causation_id: envelope.message_id,
										correlation_id: envelope.message_id,
										kind: "command.receipt",
										message_id,
										origin: "backend",
										payload: {
											journal_sequence: acceptance.event.journal_sequence,
											status: acceptance.status,
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at,
										thread_id: envelope.thread_id,
										...(envelope.agent_id === undefined
											? {}
											: { agent_id: envelope.agent_id }),
										...(envelope.run_id === undefined
											? {}
											: { run_id: envelope.run_id }),
									});

									const latest = yield* Ref.get(state);

									if (latest._tag === "Ready") {
										yield* journal
											.ReadReplay({
												after_journal_sequence:
													latest.delivered_journal_sequence,
											})
											.pipe(
												Effect.flatMap(DeliverLiveEvents),
												Effect.catch(() =>
													EnqueueError(
														latest,
														"journal.replay_failed",
														"Live journal delivery could not be resumed.",
														true,
														envelope.message_id,
													),
												),
											);
									}
								}),
						}),
					);
				};

				const HandleReadyEnvelope = (
					envelope: Exclude<InboundControlEnvelope, HelloEnvelope>,
					current: ReadyState,
				) => {
					switch (envelope.kind) {
						case "command":
							return HandleCommand(envelope);
						case "thread.list.query":
							return HandleQuery(envelope, current);
						case "thread.retention.query":
							return HandleRetentionQuery(envelope, current);
						case "thread.retention.update":
							return HandleRetentionUpdate(envelope);
						case "workspace.file.read.query":
							return HandleWorkspaceFileReadQuery(envelope, current);
						case "workspace.change.list.query":
							return HandleWorkspaceChangeListQuery(envelope, current);
						case "workspace.change.diff.query":
							return HandleWorkspaceChangeDiffQuery(envelope, current);
						case "git.workspace.query":
							return HandleGitWorkspaceQuery(envelope, current);
						case "git.diff.query":
							return HandleGitDiffQuery(envelope, current);
						case "git.index.stage.request":
						case "git.index.unstage.request":
						case "git.mutation.resolve":
							return HandleGitMutation(envelope);
						case "workspace.file.replace":
						case "workspace.change.review":
						case "workspace.change.rollback":
							return HandleWorkspaceMutation(envelope);
						case "guidance.query":
							return HandleGuidanceQuery(envelope, current);
						case "guidance.update":
						case "guidance.selection":
						case "guidance.drift.resolve":
						case "guidance.sync.retry":
							return HandleGuidanceMutation(envelope);
						case "model_behaviour.query":
							return HandleModelBehaviourQuery(envelope, current);
						case "model_behaviour.update":
						case "model_behaviour.drift.resolve":
						case "model_behaviour.sync.retry":
							return HandleModelBehaviourMutation(envelope);
						case "thread.work.query":
							return HandleWorkQuery(envelope, current);
						case "terminal.list.query":
							return HandleTerminalListQuery(envelope, current);
						case "orchestration.graph.query":
							return HandleGraphQuery(envelope, current);
						case "subscribe":
							return HandleSubscribe(envelope, current);
						case "unsubscribe":
							return HandleUnsubscribe(envelope, current);
						case "ack":
							return HandleAck(envelope, current);
						case "replay":
							return HandleReplay(envelope, current);
						case "heartbeat.pong":
							return HandlePong(envelope, current);
						default: {
							const unhandled: never = envelope;

							return Effect.die(`Unhandled inbound envelope: ${unhandled}`);
						}
					}
				};

				const HandleEnvelope = (dispatch: ProtocolRouterInboundDispatch) =>
					Effect.gen(function* () {
						const envelope =
							dispatch._tag === "Command" ? dispatch.command : dispatch.envelope;
						const current = yield* Ref.get(state);

						if (current._tag === "Closed" || current._tag === "Rejected") {
							return;
						}

						if (current._tag === "AwaitingHello") {
							if (envelope.kind !== "hello") {
								yield* EnqueueError(
									current,
									"protocol.handshake_required",
									"A hello frame is required before negotiated traffic.",
									false,
									envelope.message_id,
								);

								return;
							}

							return yield* HandleHello(envelope, current);
						}

						if (envelope.kind === "hello") {
							yield* EnqueueError(
								current,
								"protocol.already_negotiated",
								"The connection has already negotiated a protocol version.",
								false,
								envelope.message_id,
							);

							return;
						}

						yield* HandleReadyEnvelope(envelope, current);
					});

				const Receive = (input: unknown) =>
					Semaphore.withPermit(receive_lock)(
						Effect.gen(function* () {
							const current = yield* Ref.get(state);

							if (current._tag === "Closed" || current._tag === "Rejected") {
								return;
							}

							const last_activity_ms = yield* Clock.currentTimeMillis;

							yield* Ref.set(state, { ...current, last_activity_ms });

							return yield* router.ClassifyInbound(input).pipe(
								Effect.flatMap(HandleEnvelope),
								Effect.catch(() =>
									EnqueueError(
										current,
										"protocol.invalid_message",
										"The message does not match the Artisan control protocol.",
										false,
									),
								),
							);
						}),
					);

				const DeliverJournalTail = Semaphore.withPermit(receive_lock)(
					Effect.gen(function* () {
						const current = yield* Ref.get(state);

						if (current._tag !== "Ready") {
							return;
						}

						return yield* journal
							.ReadReplay({
								after_journal_sequence: current.delivered_journal_sequence,
							})
							.pipe(
								Effect.flatMap(DeliverLiveEvents),
								Effect.catch(() =>
									EnqueueError(
										current,
										"journal.replay_failed",
										"Live journal delivery could not be resumed.",
										true,
									),
								),
							);
					}),
				);

				const JournalTail = Deferred.await(connection_ready).pipe(
					Effect.andThen(
						Effect.forever(
							PubSub.take(journal_subscription).pipe(
								Effect.andThen(DeliverJournalTail),
							),
						),
					),
				);

				const HeartbeatTick = Semaphore.withPermit(receive_lock)(
					Effect.gen(function* () {
						const current = yield* Ref.get(state);
						const now = yield* Clock.currentTimeMillis;

						if (current._tag === "Closed") {
							return false;
						}

						if (current._tag === "Rejected" || current._tag === "AwaitingHello") {
							return now - current.last_activity_ms >= options.heartbeat_timeout_ms;
						}

						if (current.pending_heartbeat) {
							return now >= current.pending_heartbeat.deadline_ms;
						}

						if (now - current.last_activity_ms < options.heartbeat_interval_ms) {
							return false;
						}

						const message_id = yield* metadata.MakeId("heartbeat");
						const nonce = yield* metadata.MakeId("heartbeat");
						const sent_at = yield* metadata.Now;

						yield* Enqueue({
							kind: "heartbeat.ping",
							message_id,
							origin: "backend",
							payload: { nonce },
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
						yield* Ref.set(state, {
							...current,
							pending_heartbeat: {
								deadline_ms: now + options.heartbeat_timeout_ms,
								message_id,
								nonce,
							},
						});

						return false;
					}),
				).pipe(
					Effect.flatMap((should_close) => (should_close ? RequestClose : Effect.void)),
				);
				const Heartbeat = Effect.forever(
					Effect.sleep(options.heartbeat_interval_ms).pipe(Effect.andThen(HeartbeatTick)),
				);

				yield* Effect.forkIn(JournalTail, connection_scope);
				yield* Effect.forkIn(Heartbeat, connection_scope);

				return {
					Close,
					Closed: Deferred.await(closed),
					Outbound: Stream.fromQueue(outbound).pipe(
						Stream.catchCauseIf(Cause.hasInterruptsOnly, () => Stream.empty),
					),
					Receive,
				};
			});

			return { Open };
		}),
	);
}
