import { Effect } from "effect";

import type { SubscribeEnvelope } from "@artisan/protocol";

import { AgentGraphOrchestrator } from "../../orchestration/agent-graph-orchestrator";
import { ThreadReadModel } from "../../persistence/thread-read-model";
import type {
	OrchestrationGraphProjectionSubscription,
	OrchestrationGroupListProjectionSubscription,
	ReadyState,
} from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";
import { ConnectionProjectionRuntime } from "./runtime";

export const MakeOrchestrationProjectionHandler = Effect.gen(function* () {
	const graph = yield* AgentGraphOrchestrator;
	const threads = yield* ThreadReadModel;
	const runtime = yield* ConnectionProjectionRuntime;
	const { EnqueueError } = yield* ConnectionSubscriptionControl;

	const Handle = (subscribe: SubscribeEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			if (
				subscribe.payload.type !== "orchestration.graph" &&
				subscribe.payload.type !== "orchestration.group.list" &&
				subscribe.payload.type !== "thread.list"
			)
				return;
			const subscription_id = subscribe.subscription_id;
			const correlation_id = subscribe.message_id;

			if (subscribe.payload.type === "orchestration.group.list") {
				const { thread_id, include_terminal } = subscribe.payload;
				yield* graph.ListGroupsSnapshot(thread_id, include_terminal).pipe(
					Effect.flatMap(({ groups, journal_sequence }) => {
						const stream_id = `projection:orchestration.group.list:${thread_id}:${subscription_id}`;
						const subscription: OrchestrationGroupListProjectionSubscription = {
							_tag: "orchestration.group.list",
							thread_id,
							include_terminal,
							journal_sequence,
							sequence: 0,
							stream_id,
						};
						return runtime.Start(
							correlation_id,
							subscription_id,
							current,
							subscription,
							({ message_id, sent_at }) => ({
								journal_sequence,
								kind: "orchestration.group.list.snapshot",
								message_id,
								origin: "backend",
								payload: { groups, journal_sequence },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id,
							}),
						);
					}),
					Effect.catch(() =>
						EnqueueError(
							current,
							"projection.unavailable",
							"The orchestration group list could not be read.",
							true,
							correlation_id,
						),
					),
				);
				return;
			}

			if (subscribe.payload.type === "orchestration.graph") {
				const group_id = subscribe.payload.group_id;
				yield* graph.GetGraph(group_id).pipe(
					Effect.flatMap((projection) => {
						const stream_id = `projection:orchestration.graph:${group_id}:${subscription_id}`;
						const subscription: OrchestrationGraphProjectionSubscription = {
							_tag: "orchestration.graph",
							group_id,
							sequence: 0,
							stream_id,
						};
						return runtime.Start(
							correlation_id,
							subscription_id,
							current,
							subscription,
							({ message_id, sent_at }) => ({
								journal_sequence: projection.journal_sequence,
								kind: "orchestration.graph.snapshot",
								message_id,
								origin: "backend",
								payload: { graph: projection },
								protocol_version: 1,
								schema_version: 1,
								sent_at,
								sequence: 0,
								stream_id,
								subscription_id,
							}),
						);
					}),
					Effect.catch(() =>
						EnqueueError(
							current,
							"projection.unavailable",
							"The orchestration graph projection could not be read.",
							true,
							correlation_id,
						),
					),
				);
				return;
			}

			yield* threads.Snapshot().pipe(
				Effect.flatMap((snapshot) => {
					const stream_id = `projection:thread.list:${subscription_id}`;
					return runtime.Start(
						correlation_id,
						subscription_id,
						current,
						{ _tag: "thread.list", sequence: 0, stream_id },
						({ message_id, sent_at }) => ({
							journal_sequence: snapshot.journal_sequence,
							kind: "thread.list.snapshot",
							message_id,
							origin: "backend",
							payload: { threads: snapshot.threads },
							protocol_version: 1,
							schema_version: 1,
							sent_at,
							sequence: 0,
							stream_id,
							subscription_id,
						}),
					);
				}),
				Effect.catch(() =>
					EnqueueError(
						current,
						"projection.unavailable",
						"The thread projection could not be read.",
						true,
						correlation_id,
					),
				),
			);
		});

	return { Handle };
});
