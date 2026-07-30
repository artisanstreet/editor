import {
	Cause,
	Clock,
	Context,
	Deferred,
	Effect,
	Exit,
	Layer,
	PubSub,
	Queue,
	Ref,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	SupportedProtocolVersions,
	SnowflakeId,
	type EventEnvelope,
	type HelloEnvelope,
	OutboundControlEnvelope,
	type PreNegotiationProtocolErrorEnvelope,
	type ProtocolErrorDetail,
	type ProtocolErrorEnvelope,
} from "@artisan/protocol";

import { ProjectDirectoryService } from "../projects/project-directory-service";
import { ProjectCatalog } from "../projects/project-catalog";
import { NodeGitCommandExecutorLive } from "../git/executor";
import { RepositoryService, RepositoryServiceLive } from "../git/repository-service";
import { CapabilityRepository } from "../marketplace/capabilities/repository";
import { CapabilityService } from "../marketplace/capabilities/service";
import { CapabilityOAuthLifecycle } from "../marketplace/capabilities/oauth-lifecycle";
import { CapabilityMirrorService } from "../marketplace/capabilities/provider-mirrors";
import { RoutineService } from "../marketplace/routines/service";
import { RoutineRepository } from "../marketplace/routines/repository";

import { GitService } from "../git/service";
import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { JournalNotifier } from "../persistence/journal-notifier";
import { SurfaceService } from "../surfaces/service";
import { JournalStore } from "../persistence/journal-store";
import { OrchestrationRepository } from "../persistence/orchestration/repository";
import { ThreadReadModel } from "../persistence/thread-read-model";
import { TranscriptReadModel } from "../persistence/transcript-read-model";
import { ConversationReadModel } from "../conversation/index.ts";
import { HostIdentityLive } from "../runtime/host-identity";
import { RuntimeMetadata } from "../runtime/metadata";
import { RuntimeCatalogService } from "../runtime/catalog";
import { TerminalSessionService } from "../terminal/sessions";
import { SessionDefaultsService } from "../settings/session-defaults-service";
import { WorkspaceChangeRepository } from "../workspace/changes/repository";
import { WorkspaceFileService } from "../workspace/files/service";
import { ToolControlPlane } from "../tools/tool-control-plane";
import { PreviewCoordinator } from "../preview/coordinator";
import {
	DecodeProtocolConnectionOptions,
	DefaultProtocolConnectionOptions,
	type ProtocolConnection,
	type ProtocolConnectionOptions,
} from "./connection";
import { ProtocolRouter, type ProtocolRouterInboundDispatch } from "./router";
import {
	ApplyEventCursors,
	type AwaitingHelloState,
	type ConnectionState,
	CursorsToRecord,
	LatestJournalSequence,
	type ReadyState,
	RecordToCursors,
} from "./connection-state";
import { MakeSettingsMutationHandler } from "./rpc/mutation-handlers/settings";
import { MakeEngineUsageQueryHandler } from "./rpc/query-handlers/engine-usage";
import { MakeHostIdentityQueryHandler } from "./rpc/query-handlers/host-identity";
import { MakeMarketplaceQueryHandler } from "./rpc/query-handlers/marketplace";
import { ConnectionResponseSink } from "./rpc/query-handlers/project";
import { MakeThreadQueryHandler } from "./rpc/query-handlers/thread";
import { MakeWorkspaceInspectionQueryHandler } from "./rpc/query-handlers/workspace-inspection";
import { MakeReadyEnvelopeDispatch, ReadyHandlerFactory } from "./rpc/ready-dispatch";
import { ReadyConnectionRuntime } from "./rpc/ready-mutations";
import { ConnectionSubscriptionControl } from "./subscriptions/control";
import {
	ConnectionConversationDelivery,
	MakeConnectionConversationDelivery,
} from "./subscriptions/conversation-delivery";
import { MakeLiveEventDelivery } from "./subscriptions/live-events";
import {
	ConnectionProjectionRuntime,
	MakeConnectionProjectionRuntime,
} from "./subscriptions/runtime";

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
			const surfaces = yield* SurfaceService;
			const journal = yield* JournalStore;
			const metadata = yield* RuntimeMetadata;
			const snowflake_id = yield* SnowflakeId;
			const notifier = yield* JournalNotifier;
			const router = yield* ProtocolRouter;
			const orchestration = yield* OrchestrationRepository;
			const terminals = yield* TerminalSessionService;
			const tools = yield* ToolControlPlane;
			const thread_read_model = yield* ThreadReadModel;
			const transcript_read_model = yield* TranscriptReadModel;
			const conversation_read_model = yield* ConversationReadModel;
			const project_catalog = yield* ProjectCatalog;
			const project_directories = yield* ProjectDirectoryService;
			const repository_service = yield* RepositoryService;
			const session_defaults = yield* SessionDefaultsService;
			const runtime_catalog = yield* RuntimeCatalogService;
			const workspace_changes = yield* WorkspaceChangeRepository;
			const workspace_files = yield* WorkspaceFileService;
			const previews = yield* PreviewCoordinator;
			const routines = yield* RoutineService;
			const routine_repository = yield* RoutineRepository;
			const capabilities = yield* CapabilityService;
			const capability_repository = yield* CapabilityRepository;
			const capability_oauth = yield* CapabilityOAuthLifecycle;
			const capability_mirrors = yield* CapabilityMirrorService;
			const HandleSettingsMutation = yield* MakeSettingsMutationHandler;
			const HandleEngineUsageQuery = yield* MakeEngineUsageQueryHandler;
			const HandleHostIdentityQuery = yield* MakeHostIdentityQueryHandler;
			const HandleMarketplaceQuery = yield* MakeMarketplaceQueryHandler;
			const HandleThreadQuery = yield* MakeThreadQueryHandler;
			const HandleWorkspaceInspectionQuery = yield* MakeWorkspaceInspectionQueryHandler;
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
				const project_subscription = yield* project_catalog.Subscribe.pipe(
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
				const connection_subscription_control = ConnectionSubscriptionControl.of({
					state,
					Enqueue,
					EnqueueError,
				});
				const projection_runtime = yield* MakeConnectionProjectionRuntime.pipe(
					Effect.provideService(
						ConnectionSubscriptionControl,
						connection_subscription_control,
					),
					Effect.provideService(RuntimeMetadata, metadata),
				);
				const conversation_delivery = yield* MakeConnectionConversationDelivery.pipe(
					Effect.provideService(
						ConnectionSubscriptionControl,
						connection_subscription_control,
					),
					Effect.provideService(ConversationReadModel, conversation_read_model),
					Effect.provideService(RuntimeMetadata, metadata),
				);
				const { DeliverLiveEvents } = yield* MakeLiveEventDelivery.pipe(
					Effect.provideService(
						ConnectionSubscriptionControl,
						connection_subscription_control,
					),
					Effect.provideService(ConnectionConversationDelivery, conversation_delivery),
					Effect.provideService(AgentGraphOrchestrator, graph),
					Effect.provideService(OrchestrationRepository, orchestration),
					Effect.provideService(RuntimeMetadata, metadata),
					Effect.provideService(SurfaceService, surfaces),
					Effect.provideService(ThreadReadModel, thread_read_model),
					Effect.provideService(TranscriptReadModel, transcript_read_model),
					Effect.provideService(WorkspaceChangeRepository, workspace_changes),
				);
				const ready_connection_runtime = ReadyConnectionRuntime.of({
					state,
					Enqueue,
					EnqueueError,
					DeliverLiveEvents,
				});
				const ready_dispatch_connection = MakeReadyEnvelopeDispatch.pipe(
					Effect.provideService(ReadyConnectionRuntime, {
						...ready_connection_runtime,
					}),
					Effect.provideService(ReadyHandlerFactory, {
						Make: Effect.succeed({
							engine_usage: HandleEngineUsageQuery,
							host_identity: HandleHostIdentityQuery,
							marketplace: HandleMarketplaceQuery,
							settings: HandleSettingsMutation,
							thread: HandleThreadQuery,
							workspace_inspection: HandleWorkspaceInspectionQuery,
						}),
					}),
					Effect.provideService(ConnectionResponseSink, {
						Enqueue,
						EnqueueError,
					}),
					Effect.provideService(
						ConnectionSubscriptionControl,
						connection_subscription_control,
					),
					Effect.provideService(ConnectionProjectionRuntime, projection_runtime),
					Effect.provideService(ConnectionConversationDelivery, conversation_delivery),
					Effect.provideService(AgentGraphOrchestrator, graph),
					Effect.provideService(CapabilityMirrorService, capability_mirrors),
					Effect.provideService(CapabilityOAuthLifecycle, capability_oauth),
					Effect.provideService(CapabilityRepository, capability_repository),
					Effect.provideService(CapabilityService, capabilities),
					Effect.provideService(ConversationReadModel, conversation_read_model),
					Effect.provideService(GitService, git),
				);
				const ready_dispatch_domain = ready_dispatch_connection.pipe(
					Effect.provideService(ProtocolRouter, router),
					Effect.provideService(JournalStore, journal),
					Effect.provideService(OrchestrationRepository, orchestration),
					Effect.provideService(PreviewCoordinator, previews),
					Effect.provideService(ProjectCatalog, project_catalog),
					Effect.provideService(ProjectDirectoryService, project_directories),
					Effect.provideService(RepositoryService, repository_service),
					Effect.provideService(RoutineRepository, routine_repository),
					Effect.provideService(RoutineService, routines),
					Effect.provideService(RuntimeMetadata, metadata),
				);
				const ready_dispatch = yield* ready_dispatch_domain.pipe(
					Effect.provideService(RuntimeCatalogService, runtime_catalog),
					Effect.provideService(SessionDefaultsService, session_defaults),
					Effect.provideService(SnowflakeId, snowflake_id),
					Effect.provideService(SurfaceService, surfaces),
					Effect.provideService(TerminalSessionService, terminals),
					Effect.provideService(ThreadReadModel, thread_read_model),
					Effect.provideService(ToolControlPlane, tools),
					Effect.provideService(TranscriptReadModel, transcript_read_model),
					Effect.provideService(WorkspaceChangeRepository, workspace_changes),
					Effect.provideService(WorkspaceFileService, workspace_files),
				);

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

						const is_fresh_session = hello.payload.resume_mode === "fresh";
						const replay = is_fresh_session
							? journal.ReadBaseline().pipe(
									Effect.map((baseline) => ({
										delivered_cursors: baseline.event_cursors,
										events: [] as ReadonlyArray<EventEnvelope>,
										journal_sequence: baseline.journal_sequence,
									})),
								)
							: journal
									.ReadReplay({
										after_journal_sequence: hello.payload.last_journal_sequence,
										stream_cursors: hello.payload.event_cursors,
									})
									.pipe(
										Effect.map((events) => ({
											delivered_cursors: RecordToCursors(
												ApplyEventCursors(
													CursorsToRecord(hello.payload.event_cursors),
													events,
												),
											),
											events,
											journal_sequence: LatestJournalSequence(
												hello.payload.last_journal_sequence,
												events,
											),
										})),
									);

						return yield* replay.pipe(
							Effect.flatMap(
								({
									delivered_cursors: current_cursors,
									events,
									journal_sequence,
								}) =>
									Effect.gen(function* () {
										const connection_id = yield* metadata.MakeId("connection");
										const stream_ticket =
											yield* metadata.MakeId("stream_ticket");
										const welcome_id = yield* metadata.MakeId("message");
										const replay_id = yield* metadata.MakeId("message");
										const sent_at = yield* metadata.Now;
										const delivered_cursors = CursorsToRecord(current_cursors);
										const ready: ReadyState = {
											_tag: "Ready",
											acknowledged_cursors: is_fresh_session
												? delivered_cursors
												: CursorsToRecord(hello.payload.event_cursors),
											acknowledged_journal_sequence: is_fresh_session
												? journal_sequence
												: hello.payload.last_journal_sequence,
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
												current_event_cursors: current_cursors,
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
												current_event_cursors: current_cursors,
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
									is_fresh_session
										? "protocol.bootstrap_failed"
										: "protocol.resume_invalid",
									is_fresh_session
										? "Forge could not establish a fresh client session."
										: "The supplied resume cursor does not match the journal.",
									is_fresh_session,
									hello.message_id,
								),
							),
						);
					});

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

						yield* ready_dispatch.Handle(envelope, current);
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
				yield* ready_dispatch
					.ProjectCatalogTail(project_subscription, connection_ready, receive_lock)
					.pipe(Effect.forkIn(connection_scope));
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
	).pipe(
		Layer.provide(HostIdentityLive),
		/** Repository reads run their own bounded Git process; no workspace registration. */
		Layer.provide(RepositoryServiceLive.pipe(Layer.provide(NodeGitCommandExecutorLive))),
	);
}
