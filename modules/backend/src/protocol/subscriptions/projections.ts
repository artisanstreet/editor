import { Effect, Ref } from "effect";

import type { SubscribeEnvelope, UnsubscribeEnvelope } from "@artisan/protocol";

import { RuntimeMetadata } from "../../runtime/metadata";
import type { ReadyState } from "../connection-state";
import { MakeBasicProjectionHandler } from "./basic";
import { MakeConversationProjectionHandler } from "./conversation";
import { ConnectionSubscriptionControl } from "./control";
import { MakeOrchestrationProjectionHandler } from "./orchestration";

export const MakeProjectionSubscriptionHandlers = Effect.gen(function* () {
	const metadata = yield* RuntimeMetadata;
	const { state, Enqueue, EnqueueError } = yield* ConnectionSubscriptionControl;
	const basic = yield* MakeBasicProjectionHandler;
	const conversation = yield* MakeConversationProjectionHandler;
	const orchestration = yield* MakeOrchestrationProjectionHandler;

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

			if (
				subscribe.payload.type === "conversation" ||
				subscribe.payload.type === "thread.transcript"
			) {
				yield* conversation.Handle(subscribe, current);
				return;
			}

			if (
				subscribe.payload.type === "orchestration.graph" ||
				subscribe.payload.type === "orchestration.group.list" ||
				subscribe.payload.type === "thread.list"
			) {
				yield* orchestration.Handle(subscribe, current);
				return;
			}

			yield* basic.Handle(subscribe, current);
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

			const subscriptions = { ...current.subscriptions };
			delete subscriptions[unsubscribe.subscription_id];
			yield* Ref.set(state, { ...current, subscriptions });
			yield* Enqueue({
				correlation_id: unsubscribe.message_id,
				kind: "subscription.stopped",
				message_id: yield* metadata.MakeId("message"),
				origin: "backend",
				payload: {},
				protocol_version: 1,
				schema_version: 1,
				sent_at: yield* metadata.Now,
				subscription_id: unsubscribe.subscription_id,
			});
		});

	return {
		DeliverProjectCatalog: basic.DeliverProjectCatalog,
		HandleSubscribe,
		HandleUnsubscribe,
		ProjectCatalogTail: basic.ProjectCatalogTail,
	};
});
