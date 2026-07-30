import { Deferred, Effect, PubSub, Ref, Semaphore } from "effect";

import type { ProjectCatalogSnapshot, SubscribeEnvelope } from "@artisan/protocol";

import { OrchestrationRepository } from "../../persistence/orchestration/repository";
import { ProjectCatalog } from "../../projects/project-catalog";
import { SurfaceService } from "../../surfaces/service";
import { WorkspaceChangeRepository } from "../../workspace/changes/repository";
import { RuntimeMetadata } from "../../runtime/metadata";
import type {
	ReadyState,
	SurfaceListProjectionSubscription,
	SurfaceUsageProjectionSubscription,
	ThreadSessionProjectionSubscription,
	WorkspaceConflictListProjectionSubscription,
} from "../connection-state";
import { ConnectionProjectionRuntime } from "./runtime";
import { ConnectionSubscriptionControl } from "./control";

export const MakeBasicProjectionHandler = Effect.gen(function* () {
	const orchestration = yield* OrchestrationRepository;
	const project_catalog = yield* ProjectCatalog;
	const surfaces = yield* SurfaceService;
	const workspace_changes = yield* WorkspaceChangeRepository;
	const runtime = yield* ConnectionProjectionRuntime;
	const metadata = yield* RuntimeMetadata;
	const { state, Enqueue } = yield* ConnectionSubscriptionControl;

	const Handle = (subscribe: SubscribeEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			const subscription_id = subscribe.subscription_id;
			const correlation_id = subscribe.message_id;

			if (subscribe.payload.type === "thread.session") {
				const thread_id = subscribe.payload.thread_id;
				const snapshot = yield* orchestration.GetSession(thread_id);
				const stream_id = `projection:thread.session:${thread_id}:${subscription_id}`;
				const subscription: ThreadSessionProjectionSubscription = {
					_tag: "thread.session",
					thread_id,
					journal_sequence: snapshot.journal_sequence,
					sequence: 0,
					stream_id,
				};
				yield* runtime.Start(
					correlation_id,
					subscription_id,
					current,
					subscription,
					({ message_id, sent_at }) => ({
						journal_sequence: snapshot.journal_sequence,
						kind: "thread.session.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
						sequence: 0,
						stream_id,
						subscription_id,
					}),
				);
				return;
			}

			if (subscribe.payload.type === "project.list") {
				const snapshot = yield* project_catalog.Snapshot;
				const stream_id = `projection:project.list:${subscription_id}`;
				yield* runtime.Start(
					correlation_id,
					subscription_id,
					current,
					{ _tag: "project.list", sequence: 0, stream_id },
					({ message_id, sent_at }) => ({
						kind: "project.list.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
						sequence: 0,
						stream_id,
						subscription_id,
					}),
				);
				return;
			}

			if (subscribe.payload.type === "surface.list") {
				const query = subscribe.payload.query;
				const snapshot = yield* surfaces.ListSnapshot({
					thread_id: query.thread_id,
					...(query.run_id === undefined ? {} : { run_id: query.run_id }),
					...(query.group_id === undefined ? {} : { group_id: query.group_id }),
				});
				const stream_id = `projection:surface.list:${query.thread_id}:${subscription_id}`;
				const subscription: SurfaceListProjectionSubscription = {
					_tag: "surface.list",
					query,
					journal_sequence: snapshot.journal_sequence,
					sequence: 0,
					stream_id,
				};
				yield* runtime.Start(
					correlation_id,
					subscription_id,
					current,
					subscription,
					({ message_id, sent_at }) => ({
						journal_sequence: snapshot.journal_sequence,
						kind: "surface.list.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
						sequence: 0,
						stream_id,
						subscription_id,
					}),
				);
				return;
			}

			if (subscribe.payload.type === "workspace.conflict.list") {
				const thread_id = subscribe.payload.thread_id;
				const snapshot = yield* workspace_changes.ListConflictSnapshot(thread_id);
				const stream_id = `projection:workspace.conflict.list:${thread_id}:${subscription_id}`;
				const subscription: WorkspaceConflictListProjectionSubscription = {
					_tag: "workspace.conflict.list",
					thread_id,
					journal_sequence: snapshot.journal_sequence,
					sequence: 0,
					stream_id,
				};
				yield* runtime.Start(
					correlation_id,
					subscription_id,
					current,
					subscription,
					({ message_id, sent_at }) => ({
						journal_sequence: snapshot.journal_sequence,
						kind: "workspace.conflict.list.snapshot",
						message_id,
						origin: "backend",
						payload: snapshot,
						protocol_version: 1,
						schema_version: 1,
						sent_at,
						sequence: 0,
						stream_id,
						subscription_id,
					}),
				);
				return;
			}

			if (subscribe.payload.type !== "surface.usage.aggregate") return;
			const query = subscribe.payload.query;
			const snapshot = yield* surfaces.AggregateUsageSnapshot(query);
			const thread_id = yield* surfaces.UsageScopeThread(query);
			const stream_id = `projection:surface.usage.aggregate:${query.scope}:${query.scope_id}:${subscription_id}`;
			const subscription: SurfaceUsageProjectionSubscription = {
				_tag: "surface.usage.aggregate",
				query,
				...(thread_id === undefined ? {} : { thread_id }),
				journal_sequence: snapshot.journal_sequence,
				sequence: 0,
				stream_id,
			};
			yield* runtime.Start(
				correlation_id,
				subscription_id,
				current,
				subscription,
				({ message_id, sent_at }) => ({
					journal_sequence: snapshot.journal_sequence,
					kind: "surface.usage.aggregate.snapshot",
					message_id,
					origin: "backend",
					payload: snapshot,
					protocol_version: 1,
					schema_version: 1,
					sent_at,
					sequence: 0,
					stream_id,
					subscription_id,
				}),
			);
		});

	const DeliverProjectCatalog = (snapshot: ProjectCatalogSnapshot) =>
		Effect.gen(function* () {
			const current = yield* Ref.get(state);
			if (current._tag !== "Ready") return;
			let subscriptions = current.subscriptions;
			const sent_at = yield* metadata.Now;
			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
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
		});

	const ProjectCatalogTail = (
		project_subscription: PubSub.Subscription<ProjectCatalogSnapshot>,
		connection_ready: Deferred.Deferred<void>,
		receive_lock: Semaphore.Semaphore,
	) =>
		Deferred.await(connection_ready).pipe(
			Effect.andThen(
				Effect.forever(
					PubSub.take(project_subscription).pipe(
						Effect.flatMap((snapshot) =>
							Semaphore.withPermit(receive_lock)(DeliverProjectCatalog(snapshot)),
						),
					),
				),
			),
		);

	return { DeliverProjectCatalog, Handle, ProjectCatalogTail };
});
