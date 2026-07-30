import { Context, Effect, Ref } from "effect";

import type {
	HelloEnvelope,
	InboundControlEnvelope,
	OutboundEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import type { ReadyState } from "../connection-state";
import { MakeCapabilityMutationHandler } from "./mutation-handlers/capabilities";
import { MakePreviewMutationHandlers } from "./mutation-handlers/preview";
import { MakeRoutineMutationHandler } from "./mutation-handlers/routines";
import { MakeSettingsMutationHandler } from "./mutation-handlers/settings";
import { MakeToolMutationHandlers } from "./mutation-handlers/tools";
import { MakeControlPlaneHandlers } from "./query-handlers/control-plane";
import { MakeEngineUsageQueryHandler } from "./query-handlers/engine-usage";
import { MakeHostIdentityQueryHandler } from "./query-handlers/host-identity";
import { MakeMarketplaceQueryHandler } from "./query-handlers/marketplace";
import { MakeProjectQueryHandler } from "./query-handlers/project";
import { MakeThreadQueryHandler } from "./query-handlers/thread";
import { MakeWorkspaceInspectionQueryHandler } from "./query-handlers/workspace-inspection";
import { MakeReadyMutations, ReadyConnectionRuntime } from "./ready-mutations";
import { MakeSubscriptionControlHandlers } from "../subscriptions/control";
import { MakeProjectionSubscriptionHandlers } from "../subscriptions/projections";

export class ReadyHandlerFactory extends Context.Service<
	ReadyHandlerFactory,
	{
		readonly Make: Effect.Effect<{
			readonly engine_usage: Effect.Success<typeof MakeEngineUsageQueryHandler>;
			readonly host_identity: Effect.Success<typeof MakeHostIdentityQueryHandler>;
			readonly marketplace: Effect.Success<typeof MakeMarketplaceQueryHandler>;
			readonly settings: Effect.Success<typeof MakeSettingsMutationHandler>;
			readonly thread: Effect.Success<typeof MakeThreadQueryHandler>;
			readonly workspace_inspection: Effect.Success<
				typeof MakeWorkspaceInspectionQueryHandler
			>;
		}>;
	}
>()("Artisan/ReadyHandlerFactory") {}

export const MakeReadyEnvelopeDispatch = Effect.gen(function* () {
	const { DeliverLiveEvents, Enqueue, EnqueueError, state } = yield* ReadyConnectionRuntime;
	const handlers = yield* (yield* ReadyHandlerFactory).Make;
	const HandleSettingsMutation = handlers.settings;
	const HandleEngineUsageQuery = handlers.engine_usage;
	const HandleHostIdentityQuery = handlers.host_identity;
	const HandleMarketplaceQuery = handlers.marketplace;
	const HandleProjectQuery = yield* MakeProjectQueryHandler;
	const HandleThreadQuery = handlers.thread;
	const HandleWorkspaceInspectionQuery = handlers.workspace_inspection;
	const HandleRoutineMutation = yield* MakeRoutineMutationHandler;
	const HandleCapabilityMutation = yield* MakeCapabilityMutationHandler;
	const {
		HandleGraphQuery,
		HandleGroupListQuery,
		HandleSurfaceListQuery,
		HandleSurfaceUsageDailyQuery,
		HandleSurfaceUsageQuery,
		HandleTerminalListQuery,
		HandleToolApprovalQuery,
		HandleToolInvocationQuery,
		HandleToolRegistryQuery,
	} = yield* MakeControlPlaneHandlers;
	const {
		HandlePreviewInspection,
		HandlePreviewInspectionClose,
		HandlePreviewInspectionOpen,
		HandlePreviewLaunch,
		HandlePreviewTargetProbe,
		HandlePreviewTargetRegister,
		HandlePreviewTargetRemove,
		HandlePreviewTargetState,
	} = yield* MakePreviewMutationHandlers;
	const { HandleToolApprovalResolve, HandleToolExecute } = yield* MakeToolMutationHandlers;
	const ready_mutations = yield* MakeReadyMutations;
	const subscription_control = yield* MakeSubscriptionControlHandlers;
	const projection_subscriptions = yield* MakeProjectionSubscriptionHandlers;

	const AdaptRpcHandler =
		<Query extends { readonly message_id: string }, Result extends OutboundEnvelope>(
			handler: (query: Query) => Effect.Effect<Result, ProtocolErrorDetail>,
		) =>
		(envelope: Query, current: ReadyState) =>
			handler(envelope).pipe(
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

	const HandleThreadReadQuery = AdaptRpcHandler(HandleThreadQuery);
	const HandleWorkspaceInspectionReadQuery = AdaptRpcHandler(HandleWorkspaceInspectionQuery);
	const HandleMarketplaceReadQuery = AdaptRpcHandler(HandleMarketplaceQuery);
	const HandleHostIdentityReadQuery = AdaptRpcHandler(HandleHostIdentityQuery);
	const HandleEngineUsageReadQuery = AdaptRpcHandler(HandleEngineUsageQuery);
	const HandleSettingsMutationResult = (envelope: Parameters<typeof HandleSettingsMutation>[0]) =>
		Effect.gen(function* () {
			const result = yield* HandleSettingsMutation(envelope);
			yield* Enqueue(result.receipt);
			const latest = yield* Ref.get(state);
			const undelivered =
				latest._tag === "Ready"
					? result.events.filter(
							(event) => event.journal_sequence > latest.delivered_journal_sequence,
						)
					: [];
			if (undelivered.length > 0) yield* DeliverLiveEvents(undelivered);
		});

	const Handle = (
		envelope: Exclude<InboundControlEnvelope, HelloEnvelope>,
		current: ReadyState,
	) => {
		switch (envelope.kind) {
			case "command":
				return ready_mutations.HandleCommand(envelope);
			case "thread.create.request":
				return ready_mutations.HandleThreadCreate(envelope, current);
			case "thread.list.query":
			case "model.favorites.query":
			case "thread.retention.query":
			case "thread.work.query":
			case "thread.transcript.query":
			case "conversation.query":
			case "message.image_attachment.query":
			case "thread.session.query":
				return HandleThreadReadQuery(envelope, current);
			case "project.directory.list.query":
			case "project.directory.select":
			case "project.list.query":
			case "project.repository.query":
			case "project.diff.query":
			case "session.defaults.query":
			case "project.detach":
			case "runtime.catalog.query":
				return HandleProjectQuery(envelope, current);
			case "session.defaults.update":
				return ready_mutations.HandleSessionDefaultsUpdate(envelope);
			case "thread.retention.update":
				return ready_mutations.HandleRetentionUpdate(envelope);
			case "model.favorite.update":
				return ready_mutations.HandleModelFavoriteUpdate(envelope);
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
				return ready_mutations.HandleGitMutation(envelope);
			case "workspace.file.replace":
			case "workspace.change.review":
			case "workspace.change.rollback":
				return ready_mutations.HandleWorkspaceMutation(envelope);
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
			case "marketplace.routine.install.decision":
			case "marketplace.routine.enable":
			case "marketplace.routine.disable":
			case "marketplace.routine.remove":
			case "marketplace.routine.sync":
			case "marketplace.routine.drift.resolve":
			case "marketplace.routine.drift.overwrite.request":
			case "marketplace.routine.drift.overwrite.decision":
			case "marketplace.routine.rollback":
			case "marketplace.routine.invoke":
			case "marketplace.npx_skills.import.request":
				return HandleRoutineMutation(envelope, current);
			case "marketplace.capability.connect.request":
			case "marketplace.capability.connect.decision":
			case "marketplace.capability.start":
			case "marketplace.capability.reconnect":
			case "marketplace.capability.restart":
			case "marketplace.capability.enable":
			case "marketplace.capability.disable":
			case "marketplace.capability.remove":
			case "marketplace.capability.disconnect":
			case "marketplace.capability.uninstall":
			case "marketplace.capability.health":
			case "marketplace.capability.invoke":
			case "marketplace.capability.invoke.request":
			case "marketplace.capability.invoke.decision":
			case "marketplace.capability.sync":
			case "marketplace.capability.drift.resolve":
			case "marketplace.capability.drift.overwrite.request":
			case "marketplace.capability.drift.overwrite.decision":
			case "marketplace.capability.oauth.begin":
			case "marketplace.capability.oauth.complete":
			case "marketplace.capability.oauth.refresh":
			case "marketplace.capability.oauth.revoke":
				return HandleCapabilityMutation(envelope, current);
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
			case "surface.usage.daily.query":
				return HandleSurfaceUsageDailyQuery(envelope, current);
			case "host.identity.query":
				return HandleHostIdentityReadQuery(envelope, current);
			case "engine.usage.query":
				return HandleEngineUsageReadQuery(envelope, current);
			case "subscribe":
				return projection_subscriptions.HandleSubscribe(envelope, current);
			case "unsubscribe":
				return projection_subscriptions.HandleUnsubscribe(envelope, current);
			case "ack":
				return subscription_control.HandleAck(envelope, current);
			case "replay":
				return subscription_control.HandleReplay(envelope, current);
			case "heartbeat.pong":
				return subscription_control.HandlePong(envelope, current);
			default: {
				const exhaustive: never = envelope;
				return Effect.die(`Unhandled inbound control envelope: ${String(exhaustive)}`);
			}
		}
	};

	return {
		Handle,
		ProjectCatalogTail: projection_subscriptions.ProjectCatalogTail,
	} as const;
});
