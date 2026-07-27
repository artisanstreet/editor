import { createHash } from "node:crypto";

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
	Schema,
	Scope,
	Semaphore,
	Stream,
} from "effect";

import {
	SupportedProtocolVersions,
	SnowflakeId,
	type AckEnvelope,
	type ArtisanApprovalListQueryEnvelope,
	type ArtisanApprovalResolveEnvelope,
	type ArtisanToolExecuteEnvelope,
	type ArtisanToolInvocationListQueryEnvelope,
	type ArtisanToolRegistryListQueryEnvelope,
	type CommandEnvelope,
	type EventEnvelope,
	type GitIndexStageRequestEnvelope,
	type GitIndexUnstageRequestEnvelope,
	type GitMutationResolveEnvelope,
	type HeartbeatPongEnvelope,
	type HelloEnvelope,
	type InboundControlEnvelope,
	type OrchestrationGraphQueryEnvelope,
	type OrchestrationGroupListQueryEnvelope,
	type PreviewBrowserLaunchEnvelope,
	type PreviewInspectionEnvelope,
	type PreviewInspectionSessionCloseEnvelope,
	type PreviewInspectionSessionOpenEnvelope,
	type PreviewTargetProbeEnvelope,
	type PreviewTargetRegisterEnvelope,
	type PreviewTargetRemoveEnvelope,
	type PreviewTargetStateEnvelope,
	type ProjectDirectoryListQueryEnvelope,
	type ProjectDirectorySelectEnvelope,
	type ProjectDetachEnvelope,
	type ProjectListQueryEnvelope,
	type RuntimeCatalogQueryEnvelope,
	type SurfaceListQueryEnvelope,
	type SurfaceUsageAggregateQueryEnvelope,
	OutboundControlEnvelope,
	type PreNegotiationProtocolErrorEnvelope,
	type ProtocolErrorDetail,
	type ProtocolErrorEnvelope,
	type ReplayEnvelope,
	type StreamCursor,
	type SubscribeEnvelope,
	type ThreadCreateEnvelope,
	type ThreadListItem,
	type ThreadRetentionUpdateEnvelope,
	type TerminalListQueryEnvelope,
	type UnsubscribeEnvelope,
	type WorkspaceChangeReviewEnvelope,
	type WorkspaceChangeRollbackEnvelope,
	type WorkspaceFileReplaceEnvelope,
} from "@artisan/protocol";

import { ProjectDirectoryService } from "../projects/project-directory-service";
import { ProjectCatalog } from "../projects/project-catalog";
import { CapabilityRepository } from "../marketplace/capabilities/capability-repository";
import {
	CapabilityOAuthLifecycle,
	CapabilityService,
} from "../marketplace/capabilities/capability-service";
import { CapabilityMirrorService } from "../marketplace/capabilities/provider-mirrors";
import { RoutineService } from "../marketplace/routines/routine-service";
import { marketplace_capability_thread_id } from "../marketplace/capabilities/capability-repository";
import {
	marketplace_routine_thread_id,
	RoutineRepository,
} from "../marketplace/routines/routine-repository";

import { GitService, GitServiceError } from "../git/git-service";
import { AgentGraphOrchestrator } from "../orchestration/agent-graph-orchestrator";
import { JournalNotifier } from "../persistence/journal-notifier";
import { SurfaceService } from "../surfaces/surface-service";
import { CommandIdConflict, JournalStore } from "../persistence/journal-store";
import { OrchestrationRepository } from "../persistence/orchestration-repository";
import { ThreadReadModel } from "../persistence/thread-read-model";
import { TranscriptReadModel } from "../persistence/transcript-read-model";
import {
	ConversationReadModel,
	conversation_patch_replay_batch_size,
} from "../conversation/index.ts";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { RuntimeCatalogService } from "../runtime/runtime-catalog";
import { TerminalSessionService } from "../terminal/terminal-sessions";
import { thread_activity_kind_from_event } from "../threads/internal/thread-activity";
import { thread_retention_policy_thread_id } from "../threads/thread-retention-policy";
import { WorkspaceChangeRepository } from "../workspace/workspace-change-repository";
import {
	WorkspaceFileService,
	WorkspaceFileServiceError,
} from "../workspace/workspace-file-service";
import { ToolControlPlane } from "../tools/tool-control-plane";
import { PreviewCoordinator } from "../preview/preview-coordinator";
import { PreviewRepositoryError } from "../preview/preview-repository";
import { PreviewRuntimeError } from "../preview/preview-runtime";
import { PreviewHealthProbeError } from "../preview/preview-target";
import {
	DecodeProtocolConnectionOptions,
	DefaultProtocolConnectionOptions,
	type ProtocolConnection,
	type ProtocolConnectionOptions,
} from "./protocol-connection";
import { ProtocolRouter, type ProtocolRouterInboundDispatch } from "./protocol-router";
import { MakeSettingsMutationHandler } from "./rpc/mutation-handlers/settings";
import { MakeMarketplaceQueryHandler } from "./rpc/query-handlers/marketplace";
import { MakeThreadQueryHandler } from "./rpc/query-handlers/thread";
import { MakeWorkspaceInspectionQueryHandler } from "./rpc/query-handlers/workspace-inspection";

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
interface ProjectListProjectionSubscription {
	readonly _tag: "project.list";
	readonly sequence: number;
	readonly stream_id: string;
}

interface OrchestrationGraphProjectionSubscription {
	readonly _tag: "orchestration.graph";
	readonly group_id: string;
	readonly sequence: number;
	readonly stream_id: string;
}
interface ThreadTranscriptProjectionSubscription {
	readonly _tag: "thread.transcript";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface ConversationProjectionSubscription {
	readonly _tag: "conversation";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly patch_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface OrchestrationGroupListProjectionSubscription {
	readonly _tag: "orchestration.group.list";
	readonly thread_id: string;
	readonly include_terminal: boolean;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface ThreadSessionProjectionSubscription {
	readonly _tag: "thread.session";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface SurfaceListProjectionSubscription {
	readonly _tag: "surface.list";
	readonly query: import("@artisan/protocol").SurfaceListQuery;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface SurfaceUsageProjectionSubscription {
	readonly _tag: "surface.usage.aggregate";
	readonly thread_id?: string;
	readonly query: import("@artisan/protocol").SurfaceUsageAggregateQuery;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}
interface WorkspaceConflictListProjectionSubscription {
	readonly _tag: "workspace.conflict.list";
	readonly thread_id: string;
	readonly journal_sequence: number;
	readonly sequence: number;
	readonly stream_id: string;
}

type ProjectionSubscription =
	| OrchestrationGraphProjectionSubscription
	| ThreadTranscriptProjectionSubscription
	| ConversationProjectionSubscription
	| OrchestrationGroupListProjectionSubscription
	| ThreadSessionProjectionSubscription
	| SurfaceListProjectionSubscription
	| SurfaceUsageProjectionSubscription
	| WorkspaceConflictListProjectionSubscription
	| ProjectListProjectionSubscription
	| ThreadListProjectionSubscription;

const ScopeMatches = (
	left: import("@artisan/protocol").MarketplaceScope,
	right: import("@artisan/protocol").MarketplaceScope,
) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(left.kind === "workspace" &&
			right.kind === "workspace" &&
			left.workspace_id === right.workspace_id) ||
		(left.kind === "project" &&
			right.kind === "project" &&
			left.project_id === right.project_id));

const MarketplaceIntentFingerprint = (intent: unknown) =>
	createHash("sha256").update(JSON.stringify(intent)).digest("hex");

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

/** Converts preview-domain failures into stable renderer-safe protocol errors. */
function preview_error_detail(error: unknown): ProtocolErrorDetail {
	if (error instanceof PreviewRepositoryError) {
		if (error.code === "invalid")
			return {
				code: "preview.invalid",
				message: "The preview request conflicts with the durable preview state.",
				retryable: false,
			};
		if (error.code === "not_found")
			return {
				code: "preview.not_found",
				message: "The requested preview target or inspection session is unavailable.",
				retryable: false,
			};
		return {
			code: "preview.storage_unavailable",
			message: "The preview state could not be durably read or updated.",
			retryable: true,
		};
	}

	if (error instanceof PreviewHealthProbeError)
		return {
			code: "preview.health_unavailable",
			message: "The local preview health probe is currently unavailable.",
			retryable: true,
		};

	if (error instanceof PreviewRuntimeError) {
		if (error.code === "invalid_input" || error.code === "not_found")
			return {
				code: error.code === "invalid_input" ? "preview.invalid" : "preview.not_found",
				message: "The requested preview runtime resource is unavailable.",
				retryable: false,
			};
		if (error.code === "browser_unavailable")
			return {
				code: "preview.browser_unavailable",
				message: "The external browser opener is currently unavailable.",
				retryable: true,
			};
		return {
			code: "preview.connector_unavailable",
			message: "The external preview connector is currently unavailable.",
			retryable: true,
		};
	}

	return {
		code: "preview.unavailable",
		message: "The preview operation could not be completed.",
		retryable: true,
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
			const project_directories = yield* ProjectDirectoryService;
			const project_catalog = yield* ProjectCatalog;
			const runtime_catalog = yield* RuntimeCatalogService;
			const workspace_changes = yield* WorkspaceChangeRepository;
			const ReadWorkspaceConflictSnapshot = (thread_id: string) =>
				workspace_changes.ListConflictSnapshot(thread_id);
			const workspace_files = yield* WorkspaceFileService;
			const previews = yield* PreviewCoordinator;
			const routines = yield* RoutineService;
			const routine_repository = yield* RoutineRepository;
			const capabilities = yield* CapabilityService;
			const capability_repository = yield* CapabilityRepository;
			const capability_oauth = yield* CapabilityOAuthLifecycle;
			const capability_mirrors = yield* CapabilityMirrorService;
			const HandleSettingsMutation = yield* MakeSettingsMutationHandler;
			const HandleMarketplaceQuery = yield* MakeMarketplaceQueryHandler;
			const HandleThreadQuery = yield* MakeThreadQueryHandler;
			const HandleWorkspaceInspectionQuery = yield* MakeWorkspaceInspectionQueryHandler;
			const RequireRoutineScope = <Success, Error>(
				routine_id: string,
				scope: import("@artisan/protocol").MarketplaceScope,
				program: Effect.Effect<Success, Error>,
			) =>
				routine_repository.ReadDetail(routine_id).pipe(
					Effect.filterOrFail(
						(detail) => ScopeMatches(detail.scope, scope),
						() => "Routine is outside the requested Marketplace scope",
					),
					Effect.andThen(program),
				);
			const RequireCapabilityScope = <Success, Error>(
				capability_id: string,
				scope: import("@artisan/protocol").MarketplaceScope,
				program: Effect.Effect<Success, Error>,
			) =>
				capability_repository.ReadDetail(capability_id).pipe(
					Effect.filterOrFail(
						(detail) => ScopeMatches(detail.scope, scope),
						() => "Capability is outside the requested Marketplace scope",
					),
					Effect.andThen(program),
				);

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

				const EnqueueProjectionPatches = (current: ReadyState, event: EventEnvelope) =>
					Effect.gen(function* () {
						let subscriptions = current.subscriptions;
						const has_thread_list = Object.values(current.subscriptions).some(
							(subscription) => subscription._tag === "thread.list",
						);
						let thread_patch = direct_thread_list_patch_from_event(event);

						if (has_thread_list && event.payload.type === "thread.created") {
							const thread = yield* thread_read_model.Lookup(event.thread_id);
							thread_patch = Option.match(thread, {
								onNone: () => undefined,
								onSome: (item) => ({ _tag: "Upsert" as const, thread: item }),
							});
						}

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
							if (
								subscription._tag === "conversation" ||
								subscription._tag === "project.list"
							)
								continue;
							const message_id = yield* metadata.MakeId("message");
							const sequence = subscription.sequence + 1;
							let next_journal_sequence = event.journal_sequence;
							let next_usage_thread_id =
								subscription._tag === "surface.usage.aggregate"
									? subscription.thread_id
									: undefined;

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
							} else if (subscription._tag === "thread.transcript") {
								if (event.journal_sequence <= subscription.journal_sequence)
									continue;
								if (event.thread_id !== subscription.thread_id) continue;
								const snapshot = yield* transcript_read_model.Read({
									after_journal_sequence: Math.max(0, event.journal_sequence - 1),
									limit: 500,
									thread_id: event.thread_id,
								});
								if (snapshot.status !== "available") {
									yield* Enqueue({
										journal_sequence: event.journal_sequence,
										kind: "thread.transcript.snapshot",
										message_id,
										origin: "backend",
										payload: snapshot,
										protocol_version: 1,
										schema_version: 1,
										sent_at: event.sent_at,
										sequence,
										stream_id: subscription.stream_id,
										subscription_id,
									});
								} else {
									const entries = snapshot.entries.filter(
										(entry) => entry.journal_sequence <= event.journal_sequence,
									);
									if (entries.length === 0) continue;
									yield* Enqueue({
										journal_sequence: event.journal_sequence,
										kind: "thread.transcript.append",
										message_id,
										origin: "backend",
										payload: { entries },
										protocol_version: 1,
										schema_version: 1,
										sent_at: event.sent_at,
										sequence,
										stream_id: subscription.stream_id,
										subscription_id,
									});
								}
							} else if (subscription._tag === "orchestration.group.list") {
								if (event.journal_sequence <= subscription.journal_sequence)
									continue;
								if (event.thread_id !== subscription.thread_id) continue;
								if (event.payload.type === "thread.erased") {
									yield* Enqueue({
										journal_sequence: event.journal_sequence,
										kind: "orchestration.group.list.patch",
										message_id,
										origin: "backend",
										payload: {
											groups: [],
											journal_sequence: event.journal_sequence,
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at: event.sent_at,
										sequence,
										stream_id: subscription.stream_id,
										subscription_id,
									});
									subscriptions = {
										...subscriptions,
										[subscription_id]: {
											...subscription,
											journal_sequence: event.journal_sequence,
											sequence,
										},
									};
									continue;
								}
								const group_id = graph_group_id_from_event(event);
								if (!group_id) continue;
								const groups = yield* graph.ListGroups(
									subscription.thread_id,
									subscription.include_terminal,
								);
								yield* Enqueue({
									journal_sequence: event.journal_sequence,
									kind: "orchestration.group.list.patch",
									message_id,
									origin: "backend",
									payload: { groups, journal_sequence: event.journal_sequence },
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							} else if (subscription._tag === "thread.session") {
								const refreshes_thread_session =
									event.payload.type === "intake.assessed" ||
									event.payload.type === "intake.assumption_recorded" ||
									event.payload.type === "thread.auto_steer.updated" ||
									event.payload.type === "thread.session_policy.updated" ||
									event.payload.type === "thread.message_routed" ||
									event.payload.type === "thread.erased";
								if (
									event.journal_sequence <= subscription.journal_sequence ||
									event.thread_id !== subscription.thread_id ||
									!refreshes_thread_session
								)
									continue;
								const snapshot = yield* orchestration.GetSession(
									subscription.thread_id,
								);
								yield* Enqueue({
									journal_sequence: event.journal_sequence,
									kind: "thread.session.snapshot",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							} else if (subscription._tag === "surface.list") {
								if (
									event.journal_sequence <= subscription.journal_sequence ||
									event.thread_id !== subscription.query.thread_id
								)
									continue;
								const snapshot = yield* surfaces.ListSnapshot({
									thread_id: subscription.query.thread_id,
									...(subscription.query.run_id === undefined
										? {}
										: { run_id: subscription.query.run_id }),
									...(subscription.query.group_id === undefined
										? {}
										: { group_id: subscription.query.group_id }),
								});
								next_journal_sequence = snapshot.journal_sequence;
								yield* Enqueue({
									journal_sequence: snapshot.journal_sequence,
									kind: "surface.list.snapshot",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							} else if (subscription._tag === "workspace.conflict.list") {
								if (
									event.journal_sequence <= subscription.journal_sequence ||
									event.thread_id !== subscription.thread_id
								)
									continue;
								const snapshot = yield* ReadWorkspaceConflictSnapshot(
									subscription.thread_id,
								);
								next_journal_sequence = snapshot.journal_sequence;
								yield* Enqueue({
									journal_sequence: snapshot.journal_sequence,
									kind: "workspace.conflict.list.snapshot",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							} else if (subscription._tag === "surface.usage.aggregate") {
								if (event.journal_sequence <= subscription.journal_sequence)
									continue;
								const erases_usage_scope =
									event.payload.type === "thread.erased" &&
									subscription.thread_id !== undefined &&
									event.thread_id === subscription.thread_id;
								if (
									!erases_usage_scope &&
									!(yield* surfaces.UsageEventAffects(
										subscription.query,
										event.run_id,
									))
								)
									continue;
								const snapshot = yield* surfaces.AggregateUsageSnapshot(
									subscription.query,
								);
								if (!erases_usage_scope) {
									next_usage_thread_id = yield* surfaces.UsageScopeThread(
										subscription.query,
									);
								}
								next_journal_sequence = snapshot.journal_sequence;
								yield* Enqueue({
									journal_sequence: snapshot.journal_sequence,
									kind: "surface.usage.aggregate.snapshot",
									message_id,
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
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
									origin: "backend" as const,
									payload: { graph: projection },
									protocol_version: 1 as const,
									schema_version: 1 as const,
									sent_at: event.sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
							}

							const next_subscription =
								subscription._tag === "thread.transcript" ||
								subscription._tag === "orchestration.group.list" ||
								subscription._tag === "thread.session" ||
								subscription._tag === "surface.list" ||
								subscription._tag === "surface.usage.aggregate" ||
								subscription._tag === "workspace.conflict.list"
									? {
											...subscription,
											journal_sequence: next_journal_sequence,
											sequence,
										}
									: { ...subscription, sequence };
							subscriptions = {
								...subscriptions,
								[subscription_id]:
									subscription._tag === "surface.usage.aggregate"
										? {
												...next_subscription,
												...(next_usage_thread_id === undefined
													? {}
													: { thread_id: next_usage_thread_id }),
											}
										: next_subscription,
							};
						}

						return subscriptions;
					});

				const EnqueueConversationPatches = (current: ReadyState) =>
					Effect.gen(function* () {
						const maximum_batches_per_delivery = 4;
						let subscriptions = current.subscriptions;
						for (const [subscription_id, subscription] of Object.entries(
							current.subscriptions,
						)) {
							if (subscription._tag !== "conversation") continue;
							let patch_sequence = subscription.patch_sequence;
							let stream_sequence = subscription.sequence;
							let delivered_batches = 0;
							while (delivered_batches < maximum_batches_per_delivery) {
								const patches = yield* conversation_read_model.ReadPatches(
									subscription.thread_id,
									patch_sequence,
								);
								if (patches.length === 0) break;
								delivered_batches += 1;
								stream_sequence += 1;
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								const from_sequence = patch_sequence + 1;
								patch_sequence = patches.at(-1)!.sequence;
								yield* Enqueue({
									journal_sequence: current.delivered_journal_sequence,
									kind: "conversation.patch",
									message_id,
									origin: "backend",
									payload: {
										conversation_id: `conversation:${subscription.thread_id}`,
										from_sequence,
										patches,
										thread_id: subscription.thread_id,
										to_sequence: patch_sequence,
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									sequence: stream_sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
								subscriptions = {
									...subscriptions,
									[subscription_id]: {
										...subscription,
										journal_sequence: current.delivered_journal_sequence,
										patch_sequence,
										sequence: stream_sequence,
									},
								};
								if (patches.length < conversation_patch_replay_batch_size) break;
								yield* Effect.yieldNow;
							}
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

						const delivered_journal_sequence = latest_journal_sequence(
							current.delivered_journal_sequence,
							new_events,
						);
						subscriptions = yield* EnqueueConversationPatches({
							...current,
							delivered_journal_sequence,
							subscriptions,
						});
						yield* Ref.set(state, {
							...current,
							delivered_cursors: apply_event_cursors(
								current.delivered_cursors,
								new_events,
							),
							delivered_journal_sequence,
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
											delivered_cursors: record_to_cursors(
												apply_event_cursors(
													cursors_to_record(hello.payload.event_cursors),
													events,
												),
											),
											events,
											journal_sequence: latest_journal_sequence(
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
										const delivered_cursors =
											cursors_to_record(current_cursors);
										const ready: ReadyState = {
											_tag: "Ready",
											acknowledged_cursors: is_fresh_session
												? delivered_cursors
												: cursors_to_record(hello.payload.event_cursors),
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

				const HandleProjectDirectoryList = (
					query: ProjectDirectoryListQueryEnvelope,
					current: ReadyState,
				) =>
					project_directories.List(query.payload).pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "project.directory.list.query.result",
									message_id,
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"project_directory.unavailable",
								"The server directory listing could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleProjectDirectorySelect = (
					query: ProjectDirectorySelectEnvelope,
					current: ReadyState,
				) =>
					project_directories.Select(query.payload).pipe(
						Effect.flatMap(project_catalog.Attach),
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "project.directory.select.result",
									message_id,
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"project_directory.invalid",
								"The selected server directory is unavailable.",
								false,
								query.message_id,
							),
						),
					);

				const HandleProjectList = (query: ProjectListQueryEnvelope, current: ReadyState) =>
					project_catalog.Snapshot.pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "project.list.query.result",
									message_id,
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"project_catalog.unavailable",
								"The Forge project catalog could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleProjectDetach = (query: ProjectDetachEnvelope, current: ReadyState) =>
					project_catalog.Detach(query.payload.project_id).pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "project.detach.result",
									message_id,
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"project_catalog.unavailable",
								"The Forge project could not be detached.",
								true,
								query.message_id,
							),
						),
					);

				const HandleRuntimeCatalog = (
					query: RuntimeCatalogQueryEnvelope,
					current: ReadyState,
				) =>
					runtime_catalog.Get.pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "runtime.catalog.query.result",
									message_id,
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								});
							}),
						),
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"runtime_catalog.unavailable",
								"The Forge runtime catalog could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleMarketplaceResult = <Payload>(
					query: { readonly message_id: string },
					current: ReadyState,
					kind:
						| "marketplace.routine.invoke.result"
						| "marketplace.capability.invoke.result"
						| "marketplace.capability.oauth.begin.result",
					program: Effect.Effect<Payload, unknown>,
				) =>
					program.pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const candidate: unknown = {
									correlation_id: query.message_id,
									kind,
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
								};
								const response =
									yield* Schema.decodeUnknownEffect(OutboundControlEnvelope)(
										candidate,
									);
								yield* Enqueue(response);
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"marketplace.unavailable",
								"The Marketplace operation could not be completed.",
								true,
								query.message_id,
							),
						),
					);
				const HandleMarketplaceAction = (
					envelope: { readonly kind: string; readonly message_id: string },
					_current: ReadyState,
					program: Effect.Effect<unknown, unknown>,
				) =>
					program.pipe(
						Effect.andThen(journal.ReadReplay({ after_journal_sequence: 0 })),
						Effect.flatMap((events) =>
							Effect.gen(function* () {
								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload: {
										journal_sequence: latest_journal_sequence(0, events),
										status: "accepted",
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
									thread_id: envelope.kind.includes("capability")
										? marketplace_capability_thread_id
										: marketplace_routine_thread_id,
								});
							}),
						),
						Effect.catch(() =>
							Effect.gen(function* () {
								yield* Enqueue({
									causation_id: envelope.message_id,
									correlation_id: envelope.message_id,
									kind: "command.receipt",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload: {
										error: {
											code: "marketplace.action_rejected",
											message:
												"The Marketplace action was rejected before completion.",
											retryable: false,
										},
										status: "rejected",
									},
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
									thread_id: envelope.kind.includes("capability")
										? marketplace_capability_thread_id
										: marketplace_routine_thread_id,
								});
							}),
						),
					);
				const HandleRoutineInstallRequest = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.routine.install.request" }
					>,
					current: ReadyState,
				) =>
					HandleMarketplaceAction(
						envelope,
						current,
						routines.RequestInstall({
							...envelope.payload,
							operation_id: envelope.message_id,
							request_fingerprint: envelope.message_id,
						}),
					);
				const HandleRoutineApprovalDecision = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.routine.install.decision" }
					>,
					current: ReadyState,
				) =>
					HandleMarketplaceAction(
						envelope,
						current,
						routine_repository.ReadPendingInstall(envelope.payload.approval_id).pipe(
							Effect.filterOrFail(
								(request) =>
									request.approval_fingerprint ===
									envelope.payload.preview_fingerprint,
								() =>
									"Routine approval fingerprint does not match the reviewed request",
							),
							Effect.flatMap((request) =>
								routines.DecideInstall({
									approval_id: request.approval_id,
									approved: envelope.payload.approved,
									operation_id: request.operation_id,
									preview_fingerprint: request.approval_fingerprint,
									request_fingerprint: request.request_fingerprint,
									requested_by: "user",
									scope: request.preview.scope,
									source: request.preview.source,
								}),
							),
						),
					);
				const HandleRoutineInvoke = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.routine.invoke" }
					>,
					current: ReadyState,
				) =>
					RequireRoutineScope(
						envelope.payload.routine_id,
						envelope.payload.scope,
						routines.Invoke({
							...envelope.payload,
							engine_id: "codex",
							operation_id: envelope.message_id,
						}),
					).pipe(
						Effect.flatMap((payload) =>
							HandleMarketplaceResult(
								envelope,
								current,
								"marketplace.routine.invoke.result",
								Effect.succeed(payload),
							),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"marketplace.action_rejected",
								"The Marketplace action was rejected before completion.",
								false,
								envelope.message_id,
							),
						),
					);
				const HandleRoutineDriftOverwriteRequest = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.routine.drift.overwrite.request" }
					>,
					current: ReadyState,
				) => {
					const { intent_fingerprint, ...intent } = envelope.payload;
					return HandleMarketplaceAction(
						envelope,
						current,
						RequireRoutineScope(
							envelope.payload.routine_id,
							envelope.payload.scope,
							Effect.gen(function* () {
								if (MarketplaceIntentFingerprint(intent) !== intent_fingerprint)
									return yield* Effect.fail(
										"Routine drift overwrite intent fingerprint is invalid",
									);
								return yield* routine_repository.RecordPendingDriftOverwrite({
									operation_id: envelope.message_id,
									request: envelope.payload,
								});
							}),
						),
					);
				};
				const HandleRoutineDriftOverwriteDecision = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.routine.drift.overwrite.decision" }
					>,
					current: ReadyState,
				) =>
					HandleMarketplaceAction(
						envelope,
						current,
						routine_repository
							.ReadPendingDriftOverwrite(envelope.payload.approval_id)
							.pipe(
								Effect.filterOrFail(
									(record) => {
										const request = record.request;
										return (
											request.intent_fingerprint ===
												envelope.payload.intent_fingerprint &&
											request.engine_id === envelope.payload.engine_id &&
											request.observed_revision ===
												envelope.payload.observed_revision &&
											request.routine_id === envelope.payload.routine_id &&
											ScopeMatches(request.scope, envelope.payload.scope)
										);
									},
									() =>
										"Routine drift overwrite decision does not match the reviewed intent",
								),
								Effect.flatMap((record) =>
									RequireRoutineScope(
										record.request.routine_id,
										envelope.payload.scope,
										routine_repository
											.DecideDriftOverwrite({
												approval_id: envelope.payload.approval_id,
												approved: envelope.payload.approved,
												intent_fingerprint:
													envelope.payload.intent_fingerprint,
											})
											.pipe(
												Effect.flatMap((decision) =>
													decision === "denied"
														? Effect.void
														: routines.ExecuteApprovedDriftOverwrite({
																engine_id: record.request.engine_id,
																observed_revision:
																	record.request
																		.observed_revision,
																operation_id: envelope.message_id,
																routine_id:
																	record.request.routine_id,
															}),
												),
											),
									),
								),
							),
					);
				const HandleCapabilityInvoke = (
					envelope: Extract<
						InboundControlEnvelope,
						{ readonly kind: "marketplace.capability.invoke" }
					>,
					current: ReadyState,
				) =>
					RequireCapabilityScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						capabilities.Invoke({
							...envelope.payload,
							operation_id: envelope.message_id,
						}),
					).pipe(
						Effect.flatMap((payload) =>
							HandleMarketplaceResult(
								envelope,
								current,
								"marketplace.capability.invoke.result",
								Effect.succeed(payload),
							),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"marketplace.action_rejected",
								"The Marketplace action was rejected before completion.",
								false,
								envelope.message_id,
							),
						),
					);
				const HandleCapabilityInvocationApproval = (
					envelope: Extract<
						InboundControlEnvelope,
						{
							readonly kind:
								| "marketplace.capability.invoke.request"
								| "marketplace.capability.invoke.decision";
						}
					>,
					current: ReadyState,
				) =>
					RequireCapabilityScope(
						envelope.payload.capability_id,
						envelope.payload.scope,
						envelope.kind === "marketplace.capability.invoke.request"
							? capabilities.RequestInvocation({
									...envelope.payload,
									operation_id: envelope.message_id,
								})
							: capabilities.DecideInvocation(envelope.payload),
					).pipe(
						Effect.flatMap((payload) =>
							HandleMarketplaceResult(
								envelope,
								current,
								"marketplace.capability.invoke.result",
								Effect.succeed(payload),
							),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"marketplace.action_rejected",
								"The Marketplace invocation approval was rejected.",
								false,
								envelope.message_id,
							),
						),
					);
				const HandleCapabilityDriftOverwrite = (
					envelope: Extract<
						InboundControlEnvelope,
						{
							readonly kind:
								| "marketplace.capability.drift.overwrite.request"
								| "marketplace.capability.drift.overwrite.decision";
						}
					>,
					current: ReadyState,
				) =>
					HandleMarketplaceAction(
						envelope,
						current,
						RequireCapabilityScope(
							envelope.payload.capability_id,
							envelope.payload.scope,
							envelope.kind === "marketplace.capability.drift.overwrite.request"
								? capability_mirrors.RequestOverwrite({
										approval_fingerprint: envelope.payload.intent_fingerprint,
										approval_id: envelope.payload.approval_id,
										capability_id: envelope.payload.capability_id,
										engine_id: envelope.payload.engine_id,
										observed_revision: envelope.payload.observed_revision,
										operation_id: envelope.message_id,
										scope: envelope.payload.scope,
									})
								: capability_mirrors.DecideOverwrite({
										approval_fingerprint: envelope.payload.intent_fingerprint,
										approval_id: envelope.payload.approval_id,
										approved: envelope.payload.approved,
										capability_id: envelope.payload.capability_id,
										engine_id: envelope.payload.engine_id,
										observed_revision: envelope.payload.observed_revision,
										scope: envelope.payload.scope,
									}),
						),
					);
				const HandleCapabilityConnectRequest = (
					envelope: Extract<
						InboundControlEnvelope,
						{
							readonly kind:
								| "marketplace.capability.connect.request"
								| "marketplace.capability.connect.decision"
								| "marketplace.capability.start"
								| "marketplace.capability.reconnect"
								| "marketplace.capability.restart";
						}
					>,
					current: ReadyState,
				) => {
					if (envelope.kind === "marketplace.capability.connect.request")
						return HandleMarketplaceAction(
							envelope,
							current,
							capabilities
								.Preview({
									auth: envelope.payload.auth,
									scope: envelope.payload.scope,
									source: envelope.payload.source,
									transport: envelope.payload.transport,
								})
								.pipe(
									Effect.filterOrFail(
										(preview) =>
											preview.preview_fingerprint ===
											envelope.payload.preview_fingerprint,
										() =>
											"Capability preview changed; approval must be renewed",
									),
									Effect.flatMap((preview) =>
										capabilities.RequestConnect({
											approval_id: envelope.payload.approval_id,
											detail: {
												auth: envelope.payload.auth,
												compatibility: [...preview.compatibility],
												display_name: preview.candidate_name,
												enabled: true,
												health: { status: "unknown" },
												id: preview.candidate_id,
												lifecycle: "awaiting_approval",
												permissions: [...preview.permissions],
												policy: [],
												resources: [],
												scope: preview.scope,
												source: preview.source,
												status: "awaiting_approval",
												sync: [],
												tools: [...preview.tools],
												transport: preview.transport,
												...(preview.transport_policy === undefined
													? {}
													: {
															transport_policy:
																preview.transport_policy,
														}),
												trust: preview.trust,
											},
											operation_id: envelope.message_id,
											preview_fingerprint: preview.preview_fingerprint,
											request_fingerprint: envelope.message_id,
										}),
									),
								),
						);
					if (envelope.kind === "marketplace.capability.connect.decision")
						return HandleMarketplaceAction(
							envelope,
							current,
							capabilities.DecideConnect({
								approval_fingerprint: envelope.payload.preview_fingerprint,
								approval_id: envelope.payload.approval_id,
								approved: envelope.payload.approved,
							}),
						);
					return HandleMarketplaceAction(
						envelope,
						current,
						RequireCapabilityScope(
							envelope.payload.capability_id,
							envelope.payload.scope,
							capabilities.SessionAction({
								action:
									envelope.kind === "marketplace.capability.start"
										? "start"
										: envelope.kind === "marketplace.capability.restart"
											? "restart"
											: "reconnect",
								capability_id: envelope.payload.capability_id,
								operation_id: envelope.message_id,
							}),
						),
					);
				};
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

				const HandleGroupListQuery = (
					query: OrchestrationGroupListQueryEnvelope,
					current: ReadyState,
				) =>
					graph
						.ListGroupsSnapshot(query.payload.thread_id, query.payload.include_terminal)
						.pipe(
							Effect.flatMap((snapshot) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;
									yield* Enqueue({
										correlation_id: query.message_id,
										kind: "orchestration.group.list.query.result",
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
									"The orchestration group list could not be read.",
									true,
									query.message_id,
								),
							),
						);

				const HandleSurfaceListQuery = (
					query: SurfaceListQueryEnvelope,
					current: ReadyState,
				) =>
					surfaces
						.List({
							thread_id: query.payload.thread_id,
							...(query.payload.run_id === undefined
								? {}
								: { run_id: query.payload.run_id }),
							...(query.payload.group_id === undefined
								? {}
								: { group_id: query.payload.group_id }),
						})
						.pipe(
							Effect.flatMap((snapshot) =>
								Effect.gen(function* () {
									const message_id = yield* metadata.MakeId("message");
									const sent_at = yield* metadata.Now;
									yield* Enqueue({
										correlation_id: query.message_id,
										kind: "surface.list.query.result",
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
									"Surface items could not be read.",
									true,
									query.message_id,
								),
							),
						);

				const HandleSurfaceUsageQuery = (
					query: SurfaceUsageAggregateQueryEnvelope,
					current: ReadyState,
				) =>
					surfaces.AggregateUsageSnapshot(query.payload).pipe(
						Effect.flatMap((snapshot) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "surface.usage.aggregate.query.result",
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
								"Surface usage could not be read.",
								true,
								query.message_id,
							),
						),
					);

				const HandleToolRegistryQuery = (
					query: ArtisanToolRegistryListQueryEnvelope,
					current: ReadyState,
				) =>
					tools.Registry(query.payload).pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "artisan.tool.registry.list.query.result",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"artisan.tool.unavailable",
								"The built-in tool registry is unavailable.",
								true,
								query.message_id,
							),
						),
					);
				const HandleToolInvocationQuery = (
					query: ArtisanToolInvocationListQueryEnvelope,
					current: ReadyState,
				) =>
					tools.Invocations(query.payload).pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "artisan.tool.invocation.list.query.result",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"artisan.tool.unavailable",
								"Tool invocation history is unavailable.",
								true,
								query.message_id,
							),
						),
					);
				const HandleToolApprovalQuery = (
					query: ArtisanApprovalListQueryEnvelope,
					current: ReadyState,
				) =>
					tools.Approvals(query.payload).pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								yield* Enqueue({
									correlation_id: query.message_id,
									kind: "artisan.approval.list.query.result",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at: yield* metadata.Now,
								});
							}),
						),
						Effect.catch(() =>
							EnqueueError(
								current,
								"artisan.tool.unavailable",
								"Tool approvals are unavailable.",
								true,
								query.message_id,
							),
						),
					);
				const HandleToolExecute = (
					query: ArtisanToolExecuteEnvelope,
					current: ReadyState,
				) =>
					tools
						.Execute({
							...(query.agent_id === undefined ? {} : { agent_id: query.agent_id }),
							request: query.payload,
							...(query.run_id === undefined ? {} : { run_id: query.run_id }),
							thread_id: query.thread_id,
						})
						.pipe(
							Effect.flatMap(() =>
								Effect.gen(function* () {
									yield* Enqueue({
										causation_id: query.message_id,
										correlation_id: query.message_id,
										kind: "command.receipt",
										message_id: yield* metadata.MakeId("message"),
										origin: "backend",
										payload: {
											journal_sequence: yield* journal.ReadWatermark(),
											status: "accepted",
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at: yield* metadata.Now,
										thread_id: query.thread_id,
									});
								}),
							),
							Effect.catch(() =>
								EnqueueError(
									current,
									"artisan.tool.execution_failed",
									"The tool invocation could not be routed.",
									true,
									query.message_id,
								),
							),
						);
				const HandleToolApprovalResolve = (
					query: ArtisanApprovalResolveEnvelope,
					current: ReadyState,
				) =>
					tools
						.ResolveApproval({ request: query.payload, thread_id: query.thread_id })
						.pipe(
							Effect.flatMap(() =>
								Effect.gen(function* () {
									yield* Enqueue({
										causation_id: query.message_id,
										correlation_id: query.message_id,
										kind: "command.receipt",
										message_id: yield* metadata.MakeId("message"),
										origin: "backend",
										payload: {
											journal_sequence: yield* journal.ReadWatermark(),
											status: "accepted",
										},
										protocol_version: 1,
										schema_version: 1,
										sent_at: yield* metadata.Now,
										thread_id: query.thread_id,
									});
								}),
							),
							Effect.catch(() =>
								EnqueueError(
									current,
									"artisan.approval.resolve_failed",
									"The approval could not be resolved.",
									true,
									query.message_id,
								),
							),
						);
				const HandlePreview = <A>(
					envelope: { readonly message_id: string },
					current: ReadyState,
					kind:
						| "preview.asset.metadata.query.result"
						| "preview.browser.launch.result"
						| "preview.inspection.close.result"
						| "preview.inspection.inspect.result"
						| "preview.inspection.open.result"
						| "preview.rich_link.resolve.query.result"
						| "preview.target.get.query.result"
						| "preview.target.list.query.result"
						| "preview.target.mutation.result",
					operation: Effect.Effect<A, unknown, never>,
				) =>
					operation.pipe(
						Effect.flatMap((payload) =>
							Effect.gen(function* () {
								const message_id = yield* metadata.MakeId("message");
								const sent_at = yield* metadata.Now;
								const response = {
									correlation_id: envelope.message_id,
									kind,
									message_id,
									origin: "backend" as const,
									payload,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
								} as OutboundControlEnvelope;
								yield* Enqueue(response);
							}),
						),
						Effect.catch((error) => {
							const detail = preview_error_detail(error);

							return EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								envelope.message_id,
							);
						}),
					);
				const HandlePreviewTargetRegister = (
					command: PreviewTargetRegisterEnvelope,
					current: ReadyState,
				) => {
					const { source, ...registration } = command.payload;
					return HandlePreview(
						command,
						current,
						"preview.target.mutation.result",
						previews.Register({
							...registration,
							...(source === undefined ? {} : { source }),
							message_id: command.message_id,
						}),
					);
				};
				const HandlePreviewTargetProbe = (
					command: PreviewTargetProbeEnvelope,
					current: ReadyState,
				) =>
					HandlePreview(
						command,
						current,
						"preview.target.mutation.result",
						previews.Probe({
							message_id: command.message_id,
							target_id: command.payload.target_id,
						}),
					);
				const HandlePreviewTargetState = (
					command: PreviewTargetStateEnvelope,
					current: ReadyState,
				) => {
					const requested_state = command.payload.state;
					return requested_state === "removed"
						? EnqueueError(
								current,
								"preview.invalid_state",
								"Use the dedicated preview removal command.",
								false,
								command.message_id,
							)
						: previews.Get(command.payload.target_id).pipe(
								Effect.flatMap((target) =>
									HandlePreview(
										command,
										current,
										"preview.target.mutation.result",
										previews.SetState({
											message_id: command.message_id,
											state: requested_state,
											target_id: command.payload.target_id,
											thread_id: target.thread_id,
										}),
									),
								),
							);
				};
				const HandlePreviewTargetRemove = (
					command: PreviewTargetRemoveEnvelope,
					current: ReadyState,
				) =>
					previews.Get(command.payload.target_id).pipe(
						Effect.flatMap((target) =>
							HandlePreview(
								command,
								current,
								"preview.target.mutation.result",
								previews.Remove({
									message_id: command.message_id,
									target_id: command.payload.target_id,
									thread_id: target.thread_id,
								}),
							),
						),
					);
				const HandlePreviewLaunch = (
					command: PreviewBrowserLaunchEnvelope,
					current: ReadyState,
				) =>
					HandlePreview(
						command,
						current,
						"preview.browser.launch.result",
						previews.Launch({
							message_id: command.message_id,
							target_id: command.payload.target_id,
						}),
					);
				const HandlePreviewInspectionOpen = (
					command: PreviewInspectionSessionOpenEnvelope,
					current: ReadyState,
				) =>
					HandlePreview(
						command,
						current,
						"preview.inspection.open.result",
						previews.OpenInspection({
							...command.payload,
							message_id: command.message_id,
						}),
					);
				const HandlePreviewInspection = (
					command: PreviewInspectionEnvelope,
					current: ReadyState,
				) =>
					HandlePreview(
						command,
						current,
						"preview.inspection.inspect.result",
						previews.Inspect({ ...command.payload, message_id: command.message_id }),
					);
				const HandlePreviewInspectionClose = (
					command: PreviewInspectionSessionCloseEnvelope,
					current: ReadyState,
				) =>
					HandlePreview(
						command,
						current,
						"preview.inspection.close.result",
						previews.CloseInspection({
							...command.payload,
							message_id: command.message_id,
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

						if (subscribe.payload.type === "thread.session") {
							const snapshot = yield* orchestration.GetSession(
								subscribe.payload.thread_id,
							);
							const stream_id = `projection:thread.session:${subscribe.payload.thread_id}:${subscribe.subscription_id}`;
							const subscription: ThreadSessionProjectionSubscription = {
								_tag: "thread.session",
								thread_id: subscribe.payload.thread_id,
								journal_sequence: snapshot.journal_sequence,
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
							const sent_at = yield* metadata.Now;
							yield* Enqueue({
								correlation_id: subscribe.message_id,
								kind: "subscription.started",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: { stream_id },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								subscription_id: subscribe.subscription_id,
							});
							yield* Enqueue({
								journal_sequence: snapshot.journal_sequence,
								kind: "thread.session.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id: subscribe.subscription_id,
							});
							return;
						}

						if (subscribe.payload.type === "project.list") {
							const snapshot = yield* project_catalog.Snapshot;
							const stream_id = `projection:project.list:${subscribe.subscription_id}`;
							const subscription: ProjectListProjectionSubscription = {
								_tag: "project.list",
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
							const sent_at = yield* metadata.Now;
							yield* Enqueue({
								correlation_id: subscribe.message_id,
								kind: "subscription.started",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: { stream_id },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								subscription_id: subscribe.subscription_id,
							});
							yield* Enqueue({
								kind: "project.list.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id: subscribe.subscription_id,
							});
							return;
						}

						if (subscribe.payload.type === "surface.list") {
							const query = subscribe.payload.query;
							const snapshot = yield* surfaces.ListSnapshot({
								thread_id: query.thread_id,
								...(query.run_id === undefined ? {} : { run_id: query.run_id }),
								...(query.group_id === undefined
									? {}
									: { group_id: query.group_id }),
							});
							const stream_id = `projection:surface.list:${query.thread_id}:${subscribe.subscription_id}`;
							const subscription: SurfaceListProjectionSubscription = {
								_tag: "surface.list",
								query,
								journal_sequence: snapshot.journal_sequence,
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
							const sent_at = yield* metadata.Now;
							yield* Enqueue({
								correlation_id: subscribe.message_id,
								kind: "subscription.started",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: { stream_id },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								subscription_id: subscribe.subscription_id,
							});
							yield* Enqueue({
								journal_sequence: snapshot.journal_sequence,
								kind: "surface.list.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id: subscribe.subscription_id,
							});
							return;
						}

						if (subscribe.payload.type === "workspace.conflict.list") {
							const thread_id = subscribe.payload.thread_id;
							const snapshot = yield* ReadWorkspaceConflictSnapshot(thread_id);
							const stream_id = `projection:workspace.conflict.list:${thread_id}:${subscribe.subscription_id}`;
							const subscription: WorkspaceConflictListProjectionSubscription = {
								_tag: "workspace.conflict.list",
								thread_id,
								journal_sequence: snapshot.journal_sequence,
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
							const sent_at = yield* metadata.Now;
							yield* Enqueue({
								correlation_id: subscribe.message_id,
								kind: "subscription.started",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: { stream_id },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								subscription_id: subscribe.subscription_id,
							});
							yield* Enqueue({
								journal_sequence: snapshot.journal_sequence,
								kind: "workspace.conflict.list.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id: subscribe.subscription_id,
							});
							return;
						}

						if (subscribe.payload.type === "surface.usage.aggregate") {
							const query = subscribe.payload.query;
							const snapshot = yield* surfaces.AggregateUsageSnapshot(query);
							const thread_id = yield* surfaces.UsageScopeThread(query);
							const journal_sequence = snapshot.journal_sequence;
							const stream_id = `projection:surface.usage.aggregate:${query.scope}:${query.scope_id}:${subscribe.subscription_id}`;
							const subscription: SurfaceUsageProjectionSubscription = {
								_tag: "surface.usage.aggregate",
								query,
								...(thread_id === undefined ? {} : { thread_id }),
								journal_sequence,
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
							const sent_at = yield* metadata.Now;
							yield* Enqueue({
								correlation_id: subscribe.message_id,
								kind: "subscription.started",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: { stream_id },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								subscription_id: subscribe.subscription_id,
							});
							yield* Enqueue({
								journal_sequence,
								kind: "surface.usage.aggregate.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id: subscribe.subscription_id,
							});
							return;
						}

						if (subscribe.payload.type === "conversation") {
							const thread_id = subscribe.payload.thread_id;
							return yield* conversation_read_model.ReadSnapshot(thread_id).pipe(
								Effect.flatMap((availability) =>
									availability.status === "available"
										? Effect.gen(function* () {
												const started_id =
													yield* metadata.MakeId("message");
												const snapshot_id =
													yield* metadata.MakeId("message");
												const sent_at = yield* metadata.Now;
												const stream_id = `projection:conversation:${thread_id}:${subscribe.subscription_id}`;
												const subscription: ConversationProjectionSubscription =
													{
														_tag: "conversation",
														thread_id,
														journal_sequence:
															availability.snapshot.journal_sequence,
														patch_sequence:
															availability.snapshot
																.last_patch_sequence,
														sequence: 0,
														stream_id,
													};
												const registered = {
													...current,
													subscriptions: {
														...current.subscriptions,
														[subscribe.subscription_id]: subscription,
													},
												} satisfies ReadyState;
												yield* Ref.set(state, registered);
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
													journal_sequence:
														availability.snapshot.journal_sequence,
													kind: "conversation.snapshot",
													message_id: snapshot_id,
													origin: "backend",
													payload: availability.snapshot,
													protocol_version: 1,
													schema_version: 1,
													sent_at,
													sequence: 0,
													stream_id,
													subscription_id: subscribe.subscription_id,
												});
												const subscriptions =
													yield* EnqueueConversationPatches(registered);
												yield* Ref.set(state, {
													...registered,
													subscriptions,
												});
											})
										: EnqueueError(
												current,
												"projection.unavailable",
												"The conversation projection is unavailable.",
												true,
												subscribe.message_id,
											),
								),
								Effect.catch(() =>
									EnqueueError(
										current,
										"projection.unavailable",
										"The conversation projection could not be read.",
										true,
										subscribe.message_id,
									),
								),
							);
						}

						if (subscribe.payload.type === "thread.transcript") {
							const thread_id = subscribe.payload.thread_id;
							return yield* transcript_read_model
								.Read({ thread_id })
								.pipe(
									Effect.flatMap((snapshot) =>
										Effect.gen(function* () {
											const started_id = yield* metadata.MakeId("message");
											const snapshot_id = yield* metadata.MakeId("message");
											const sent_at = yield* metadata.Now;
											const stream_id = `projection:thread.transcript:${thread_id}:${subscribe.subscription_id}`;
											const subscription: ThreadTranscriptProjectionSubscription =
												{
													_tag: "thread.transcript",
													thread_id,
													journal_sequence: snapshot.journal_sequence,
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
												kind: "thread.transcript.snapshot",
												message_id: snapshot_id,
												origin: "backend",
												payload: snapshot,
												protocol_version: 1,
												schema_version: 1,
												sent_at,
												sequence: 0,
												stream_id,
												subscription_id: subscribe.subscription_id,
											});
										}),
									),
								)
								.pipe(
									Effect.catch(() =>
										EnqueueError(
											current,
											"projection.unavailable",
											"The thread transcript could not be read.",
											true,
											subscribe.message_id,
										),
									),
								);
						}

						if (subscribe.payload.type === "orchestration.group.list") {
							const { thread_id, include_terminal } = subscribe.payload;
							return yield* graph
								.ListGroupsSnapshot(thread_id, include_terminal)
								.pipe(
									Effect.flatMap(({ groups, journal_sequence }) =>
										Effect.gen(function* () {
											const started_id = yield* metadata.MakeId("message");
											const snapshot_id = yield* metadata.MakeId("message");
											const sent_at = yield* metadata.Now;
											const stream_id = `projection:orchestration.group.list:${thread_id}:${subscribe.subscription_id}`;
											const subscription: OrchestrationGroupListProjectionSubscription =
												{
													_tag: "orchestration.group.list",
													thread_id,
													include_terminal,
													journal_sequence,
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
												journal_sequence,
												kind: "orchestration.group.list.snapshot",
												message_id: snapshot_id,
												origin: "backend",
												payload: { groups, journal_sequence },
												protocol_version: 1,
												schema_version: 1,
												sent_at,
												sequence: 0,
												stream_id,
												subscription_id: subscribe.subscription_id,
											});
										}),
									),
								)
								.pipe(
									Effect.catch(() =>
										EnqueueError(
											current,
											"projection.unavailable",
											"The orchestration group list could not be read.",
											true,
											subscribe.message_id,
										),
									),
								);
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
						if (command.payload.type === "thread.create") {
							const current = yield* Ref.get(state);
							yield* EnqueueError(
								current,
								"protocol.legacy_thread_create",
								"Clients must create threads through thread.create.request so Forge owns the thread identity.",
								false,
								command.message_id,
							);
							return;
						}

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
				const HandleThreadCreate = (request: ThreadCreateEnvelope, current: ReadyState) =>
					Effect.gen(function* () {
						const existing_thread_id = yield* journal.FindCommandThreadId(
							request.message_id,
						);
						const thread_id = yield* Option.match(existing_thread_id, {
							onNone: () => snowflake_id.MakeBare,
							onSome: Effect.succeed,
						});
						const output = yield* router.RouteCommand({
							kind: "command",
							message_id: request.message_id,
							origin: request.origin,
							payload: {
								...request.payload,
								type: "thread.create",
							},
							protocol_version: request.protocol_version,
							schema_version: request.schema_version,
							sent_at: request.sent_at,
							thread_id,
						});
						const receipt = output.find(
							(envelope) => envelope.kind === "command.receipt",
						);

						if (
							receipt?.kind !== "command.receipt" ||
							receipt.payload.status === "rejected"
						) {
							const detail =
								receipt?.kind === "command.receipt" &&
								receipt.payload.status === "rejected"
									? receipt.payload.error
									: {
											code: "thread.create_failed",
											message: "Forge could not durably create the thread.",
											retryable: true,
										};
							return yield* EnqueueError(
								current,
								detail.code,
								detail.message,
								detail.retryable,
								request.message_id,
							);
						}

						const thread = yield* thread_read_model.Lookup(thread_id);
						const projection = yield* Option.match(thread, {
							onNone: () =>
								Effect.fail(
									new Error(
										"Created thread is missing from its authoritative projection",
									),
								),
							onSome: Effect.succeed,
						});
						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;
						yield* Enqueue({
							correlation_id: request.message_id,
							kind: "thread.create.result",
							message_id,
							origin: "backend",
							payload: projection,
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});

						const events = output.filter(
							(envelope): envelope is EventEnvelope => envelope.kind === "event",
						);
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
					}).pipe(
						Effect.catchCause(() =>
							EnqueueError(
								current,
								"thread.create_failed",
								"Forge could not durably create the thread.",
								true,
								request.message_id,
							),
						),
					);
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
				const HandleSettingsMutationResult = (
					envelope: Parameters<typeof HandleSettingsMutation>[0],
				) =>
					Effect.gen(function* () {
						const result = yield* HandleSettingsMutation(envelope);
						yield* Enqueue(result.receipt);

						const latest = yield* Ref.get(state);
						const undelivered =
							latest._tag === "Ready"
								? result.events.filter(
										(event) =>
											event.journal_sequence >
											latest.delivered_journal_sequence,
									)
								: [];

						if (undelivered.length > 0) {
							yield* DeliverLiveEvents(undelivered);
						}
					});
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

				const HandleThreadReadQuery = (
					envelope: Parameters<typeof HandleThreadQuery>[0],
					current: ReadyState,
				) =>
					HandleThreadQuery(envelope).pipe(
						Effect.matchEffect({
							onFailure: (detail) =>
								EnqueueError(
									current,
									detail.code,
									detail.message,
									detail.retryable,
									envelope.message_id,
								),
							onSuccess: Enqueue,
						}),
					);

				const HandleWorkspaceInspectionReadQuery = (
					envelope: Parameters<typeof HandleWorkspaceInspectionQuery>[0],
					current: ReadyState,
				) =>
					HandleWorkspaceInspectionQuery(envelope).pipe(
						Effect.matchEffect({
							onFailure: (detail) =>
								EnqueueError(
									current,
									detail.code,
									detail.message,
									detail.retryable,
									envelope.message_id,
								),
							onSuccess: Enqueue,
						}),
					);

				const HandleMarketplaceReadQuery = (
					envelope: Parameters<typeof HandleMarketplaceQuery>[0],
					current: ReadyState,
				) =>
					HandleMarketplaceQuery(envelope).pipe(
						Effect.matchEffect({
							onFailure: (detail) =>
								EnqueueError(
									current,
									detail.code,
									detail.message,
									detail.retryable,
									envelope.message_id,
								),
							onSuccess: Enqueue,
						}),
					);

				const HandleReadyEnvelope = (
					envelope: Exclude<InboundControlEnvelope, HelloEnvelope>,
					current: ReadyState,
				) => {
					switch (envelope.kind) {
						case "command":
							return HandleCommand(envelope);
						case "thread.create.request":
							return HandleThreadCreate(envelope, current);
						case "thread.list.query":
						case "thread.retention.query":
						case "thread.work.query":
						case "thread.transcript.query":
						case "conversation.query":
						case "message.image_attachment.query":
						case "thread.session.query":
							return HandleThreadReadQuery(envelope, current);
						case "project.directory.list.query":
							return HandleProjectDirectoryList(envelope, current);
						case "project.directory.select":
							return HandleProjectDirectorySelect(envelope, current);
						case "project.list.query":
							return HandleProjectList(envelope, current);
						case "project.detach":
							return HandleProjectDetach(envelope, current);
						case "runtime.catalog.query":
							return HandleRuntimeCatalog(envelope, current);
						case "thread.retention.update":
							return HandleRetentionUpdate(envelope);
						case "workspace.file.read.query":
						case "workspace.change.list.query":
						case "workspace.conflict.list.query":
						case "workspace.change.diff.query":
						case "git.workspace.query":
						case "git.diff.query":
						case "preview.target.list.query":
						case "preview.target.get.query":
						case "preview.rich_link.resolve.query":
						case "preview.asset.metadata.query":
						case "workspace.file.discovery.query":
						case "workspace.language.capabilities.query":
							return HandleWorkspaceInspectionReadQuery(envelope, current);
						case "preview.target.register":
							return HandlePreviewTargetRegister(envelope, current);
						case "preview.target.probe":
							return HandlePreviewTargetProbe(envelope, current);
						case "preview.target.state":
							return HandlePreviewTargetState(envelope, current);
						case "preview.target.remove":
							return HandlePreviewTargetRemove(envelope, current);
						case "preview.browser.launch":
							return HandlePreviewLaunch(envelope, current);
						case "preview.inspection.open":
							return HandlePreviewInspectionOpen(envelope, current);
						case "preview.inspection.inspect":
							return HandlePreviewInspection(envelope, current);
						case "preview.inspection.close":
							return HandlePreviewInspectionClose(envelope, current);
						case "git.index.stage.request":
						case "git.index.unstage.request":
						case "git.mutation.resolve":
							return HandleGitMutation(envelope);
						case "workspace.file.replace":
						case "workspace.change.review":
						case "workspace.change.rollback":
							return HandleWorkspaceMutation(envelope);
						case "guidance.query":
						case "model_behaviour.query":
						case "marketplace.routine.list.query":
						case "marketplace.routine.detail.query":
						case "marketplace.routine.install.preview":
						case "marketplace.npx_skills.discover":
						case "marketplace.capability.list.query":
						case "marketplace.capability.detail.query":
						case "marketplace.capability.connect.preview":
						case "marketplace.capability.oauth.status.query":
							return HandleMarketplaceReadQuery(envelope, current);
						case "guidance.update":
						case "guidance.selection":
						case "guidance.drift.resolve":
						case "guidance.sync.retry":
							return HandleSettingsMutationResult(envelope);
						case "marketplace.routine.install.request":
							return HandleRoutineInstallRequest(envelope, current);
						case "marketplace.routine.install.decision":
							return HandleRoutineApprovalDecision(envelope, current);
						case "marketplace.routine.enable":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.id,
									envelope.payload.scope,
									routines.Enable({
										operation_id: envelope.message_id,
										routine_id: envelope.payload.id,
									}),
								),
							);
						case "marketplace.routine.disable":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.id,
									envelope.payload.scope,
									routines.Disable({
										operation_id: envelope.message_id,
										routine_id: envelope.payload.id,
									}),
								),
							);
						case "marketplace.routine.remove":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.id,
									envelope.payload.scope,
									routines.Remove({
										operation_id: envelope.message_id,
										routine_id: envelope.payload.id,
									}),
								),
							);
						case "marketplace.routine.sync":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.id,
									envelope.payload.scope,
									routines.Sync({
										engine_id: envelope.payload.engine_id,
										operation_id: envelope.message_id,
										routine_id: envelope.payload.id,
									}),
								),
							);
						case "marketplace.routine.drift.resolve":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.routine_id,
									envelope.payload.scope,
									routines.ResolveDrift({
										...envelope.payload,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.routine.drift.overwrite.request":
							return HandleRoutineDriftOverwriteRequest(envelope, current);
						case "marketplace.routine.drift.overwrite.decision":
							return HandleRoutineDriftOverwriteDecision(envelope, current);
						case "marketplace.routine.rollback":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireRoutineScope(
									envelope.payload.routine_id,
									envelope.payload.scope,
									routines.Rollback({
										operation_id: envelope.message_id,
										rollback_id: envelope.payload.rollback_id,
										routine_id: envelope.payload.routine_id,
									}),
								),
							);
						case "marketplace.routine.invoke":
							return HandleRoutineInvoke(envelope, current);
						case "marketplace.npx_skills.import.request":
							return HandleMarketplaceAction(
								envelope,
								current,
								routines.PreviewNpxImport(envelope.payload).pipe(
									Effect.flatMap((preview) =>
										routines.RequestInstall({
											approval_id: envelope.message_id,
											operation_id: envelope.message_id,
											preview_fingerprint: preview.preview_fingerprint,
											request_fingerprint: envelope.message_id,
											requested_by: "user",
											scope: preview.scope,
											source: preview.source,
										}),
									),
								),
							);
						case "marketplace.capability.connect.request":
							return HandleCapabilityConnectRequest(envelope, current);
						case "marketplace.capability.connect.decision":
						case "marketplace.capability.start":
						case "marketplace.capability.reconnect":
						case "marketplace.capability.restart":
							return HandleCapabilityConnectRequest(envelope, current);
						case "marketplace.capability.enable":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.id,
									envelope.payload.scope,
									capabilities.Enable({
										capability_id: envelope.payload.id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.disable":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.id,
									envelope.payload.scope,
									capabilities.Disable({
										capability_id: envelope.payload.id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.remove":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.id,
									envelope.payload.scope,
									capabilities.Remove({
										capability_id: envelope.payload.id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.disconnect":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capabilities.Disconnect({
										capability_id: envelope.payload.capability_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.uninstall":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capabilities.Uninstall({
										capability_id: envelope.payload.capability_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.health":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capabilities.Health({
										capability_id: envelope.payload.capability_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.invoke":
							return HandleCapabilityInvoke(envelope, current);
						case "marketplace.capability.invoke.request":
						case "marketplace.capability.invoke.decision":
							return HandleCapabilityInvocationApproval(envelope, current);
						case "marketplace.capability.sync":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.id,
									envelope.payload.scope,
									capability_mirrors.Sync({
										capability_id: envelope.payload.id,
										engine_id: envelope.payload.engine_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.drift.resolve":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capability_mirrors.ResolveDrift({
										...envelope.payload,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.drift.overwrite.request":
						case "marketplace.capability.drift.overwrite.decision":
							return HandleCapabilityDriftOverwrite(envelope, current);
						case "marketplace.capability.oauth.begin":
							return HandleMarketplaceResult(
								envelope,
								current,
								"marketplace.capability.oauth.begin.result",
								capability_repository
									.ReadDetail(envelope.payload.capability_id)
									.pipe(
										Effect.filterOrFail(
											(detail) =>
												ScopeMatches(detail.scope, envelope.payload.scope),
											() =>
												"Capability is outside the requested Marketplace scope",
										),
										Effect.flatMap((detail) =>
											detail.auth.kind === "oauth"
												? capability_oauth
														.Begin({
															authorization_url:
																detail.auth.authorization_url,
															capability_id: detail.id,
															operation_id: envelope.message_id,
															scopes: detail.auth.scopes,
														})
														.pipe(
															Effect.filterOrFail(
																(result) =>
																	result._tag === "started",
																() =>
																	"OAuth begin result is unavailable for this retry",
															),
															Effect.map((result) => ({
																authorization_url:
																	result.authorization_url,
																continuation_reference:
																	result.state,
															})),
														)
												: Effect.fail("oauth unavailable"),
										),
									),
							);
						case "marketplace.capability.oauth.complete":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capability_oauth.Complete({
										capability_id: envelope.payload.capability_id,
										callback_reference: envelope.payload.callback_reference,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.oauth.refresh":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capability_oauth.Refresh({
										capability_id: envelope.payload.capability_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "marketplace.capability.oauth.revoke":
							return HandleMarketplaceAction(
								envelope,
								current,
								RequireCapabilityScope(
									envelope.payload.capability_id,
									envelope.payload.scope,
									capability_oauth.Revoke({
										capability_id: envelope.payload.capability_id,
										operation_id: envelope.message_id,
									}),
								),
							);
						case "model_behaviour.update":
						case "model_behaviour.drift.resolve":
						case "model_behaviour.sync.retry":
							return HandleSettingsMutationResult(envelope);
						case "terminal.list.query":
							return HandleTerminalListQuery(envelope, current);
						case "orchestration.graph.query":
							return HandleGraphQuery(envelope, current);
						case "orchestration.group.list.query":
							return HandleGroupListQuery(envelope, current);
						case "artisan.tool.registry.list.query":
							return HandleToolRegistryQuery(envelope, current);
						case "artisan.tool.invocation.list.query":
							return HandleToolInvocationQuery(envelope, current);
						case "artisan.approval.list.query":
							return HandleToolApprovalQuery(envelope, current);
						case "artisan.tool.execute":
							return HandleToolExecute(envelope, current);
						case "artisan.approval.resolve":
							return HandleToolApprovalResolve(envelope, current);
						case "surface.list.query":
							return HandleSurfaceListQuery(envelope, current);
						case "surface.usage.aggregate.query":
							return HandleSurfaceUsageQuery(envelope, current);
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
							const exhaustive: never = envelope;
							return Effect.die(
								`Unhandled inbound control envelope: ${String(exhaustive)}`,
							);
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

				const DeliverProjectCatalog = (
					snapshot: import("@artisan/protocol").ProjectCatalogSnapshot,
				) =>
					Semaphore.withPermit(receive_lock)(
						Effect.gen(function* () {
							const current = yield* Ref.get(state);
							if (current._tag !== "Ready") return;
							let subscriptions = current.subscriptions;
							const sent_at = yield* metadata.Now;
							for (const [subscription_id, subscription] of Object.entries(
								current.subscriptions,
							)) {
								if (subscription._tag !== "project.list") continue;
								const sequence = subscription.sequence + 1;
								yield* Enqueue({
									kind: "project.list.updated",
									message_id: yield* metadata.MakeId("message"),
									origin: "backend",
									payload: snapshot,
									protocol_version: 1,
									schema_version: 1,
									sent_at,
									sequence,
									stream_id: subscription.stream_id,
									subscription_id,
								});
								subscriptions = {
									...subscriptions,
									[subscription_id]: { ...subscription, sequence },
								};
							}
							yield* Ref.set(state, { ...current, subscriptions });
						}),
					);

				const ProjectCatalogTail = Deferred.await(connection_ready).pipe(
					Effect.andThen(
						Effect.forever(
							PubSub.take(project_subscription).pipe(
								Effect.flatMap(DeliverProjectCatalog),
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
				yield* Effect.forkIn(ProjectCatalogTail, connection_scope);
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
