import { Cause, Deferred, Effect } from "effect";

import type { SubscribeEnvelope, UnsubscribeEnvelope } from "@artisan/protocol";

import { RuntimeMetadata } from "../../runtime/metadata";
import { OrchestrationRecoveryGate } from "../../orchestration/recovery-gate";
import type { PendingSubscription, ReadyState } from "../connection-state";
import { MakeBasicProjectionHandler } from "./basic";
import { MakeConversationProjectionHandler } from "./conversation";
import { ConnectionSubscriptionControl, type SubscriptionControlHandlers } from "./control";
import { MakeOrchestrationProjectionHandler } from "./orchestration";

export const MakeProjectionSubscriptionHandlers = (
	subscription_control: SubscriptionControlHandlers,
) =>
	Effect.gen(function* () {
		const metadata = yield* RuntimeMetadata;
		const recovery_gate = yield* OrchestrationRecoveryGate;
		const { Enqueue, EnqueueError } = yield* ConnectionSubscriptionControl;
		const { ClaimCancellation, ClaimPending, FinalizePending, PublishTerminal } =
			subscription_control;
		const basic = yield* MakeBasicProjectionHandler;
		const conversation = yield* MakeConversationProjectionHandler;
		const orchestration = yield* MakeOrchestrationProjectionHandler;

		const HandleClaimedSubscribe = (
			subscribe: SubscribeEnvelope,
			current: ReadyState,
			pending: PendingSubscription | undefined,
		) =>
			Effect.gen(function* () {
				if (pending === undefined) {
					yield* EnqueueError(
						current,
						"subscription.already_exists",
						"The subscription id is already active.",
						false,
						subscribe.message_id,
					);
					return;
				}

				const handler =
					subscribe.payload.type === "conversation" ||
					subscribe.payload.type === "thread.transcript"
						? conversation.Handle(subscribe, current, pending)
						: subscribe.payload.type === "orchestration.graph" ||
							  subscribe.payload.type === "orchestration.group.list" ||
							  subscribe.payload.type === "thread.list"
							? orchestration.Handle(subscribe, current, pending)
							: basic.Handle(subscribe, current, pending);
				const admission = [
					"thread.list",
					"conversation",
					"thread.transcript",
					"thread.work",
					"surface.list",
					"surface.usage.aggregate",
					"orchestration.graph",
					"orchestration.group.list",
				].includes(subscribe.payload.type)
					? recovery_gate.Await.pipe(
							Effect.matchCauseEffect({
								onFailure: (cause) =>
									Cause.hasInterruptsOnly(cause)
										? Effect.interrupt
										: PublishTerminal(
												subscribe.subscription_id,
												pending,
												EnqueueError(
													current,
													"orchestration.recovery_unavailable",
													"Interrupted work is still unavailable. Please retry.",
													true,
													subscribe.message_id,
												),
											),
								onSuccess: () => handler,
							}),
						)
					: handler;
				yield* Effect.raceFirst(
					admission,
					Deferred.await(pending.cancelled).pipe(Effect.andThen(Effect.interrupt)),
				).pipe(Effect.ensuring(FinalizePending(subscribe.subscription_id, pending)));
			});

		const HandleUnsubscribe = (
			unsubscribe: UnsubscribeEnvelope,
			current: ReadyState,
			claim: PendingSubscription | undefined,
		) =>
			Effect.gen(function* () {
				if (claim === undefined) {
					yield* EnqueueError(
						current,
						"subscription.not_found",
						"The subscription id is not active.",
						false,
						unsubscribe.message_id,
					);
					return;
				}

				yield* PublishTerminal(
					unsubscribe.subscription_id,
					claim,
					Effect.gen(function* () {
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
					}),
				).pipe(Effect.asVoid);
			});

		return {
			ClaimSubscribe: ClaimPending,
			ClaimUnsubscribe: ClaimCancellation,
			DeliverProjectCatalog: basic.DeliverProjectCatalog,
			HandleClaimedSubscribe,
			HandleClaimedUnsubscribe: HandleUnsubscribe,
			HandleUnsubscribe,
			ProjectCatalogTail: basic.ProjectCatalogTail,
		};
	});
