import { Effect, Option, Ref } from "effect";

import type { SubscribeEnvelope } from "@artisan/protocol";

import { ConversationReadModel } from "../../conversation/index.ts";
import { TranscriptReadModel } from "../../persistence/transcript-read-model";
import { MergeOwnedSubscriptionUpdates } from "../connection-state";
import type {
	ConversationProjectionSubscription,
	PendingSubscription,
	ReadyState,
	ThreadTranscriptProjectionSubscription,
} from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";
import { ConnectionConversationDelivery } from "./conversation-delivery";
import { ConnectionProjectionRuntime } from "./runtime";

export const MakeConversationProjectionHandler = Effect.gen(function* () {
	const conversation = yield* ConversationReadModel;
	const transcript = yield* TranscriptReadModel;
	const runtime = yield* ConnectionProjectionRuntime;
	const { EnqueuePatches } = yield* ConnectionConversationDelivery;
	const {
		state,
		EnqueueError,
		PublishClaim: PublishClaimOption,
		WithDeliveryAdmission: WithDeliveryAdmissionOption,
	} = yield* ConnectionSubscriptionControl;
	const WithDeliveryAdmission = WithDeliveryAdmissionOption ?? ((effect) => effect);
	const PublishClaim =
		PublishClaimOption ??
		(<A, E, R>(
			_subscription_id: string,
			_claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => publication.pipe(Effect.map(Option.some)));

	/**
	 * Snapshot read and registration hold delivery admission as one unit: a
	 * subscription must either be registered before an event is admitted (and
	 * receive its projection patch) or read its snapshot afterwards (and have
	 * the snapshot already contain it). Unlocked, an event committed during
	 * the read is admitted past a not-yet-registered subscription and no
	 * later wake replays it.
	 */
	const Handle = (
		subscribe: SubscribeEnvelope,
		current: ReadyState,
		claim: PendingSubscription,
	) =>
		WithDeliveryAdmission(
			Effect.gen(function* () {
				if (
					subscribe.payload.type !== "conversation" &&
					subscribe.payload.type !== "thread.transcript"
				)
					return;
				const subscription_id = subscribe.subscription_id;
				const correlation_id = subscribe.message_id;
				const thread_id = subscribe.payload.thread_id;
				const EnqueueProjectionError = (message: string) =>
					PublishClaim(
						subscription_id,
						claim,
						EnqueueError(
							current,
							"projection.unavailable",
							message,
							true,
							correlation_id,
						),
					).pipe(Effect.asVoid);

				if (subscribe.payload.type === "conversation") {
					const cursor = subscribe.payload.cursor;
					if (cursor) {
						const cursor_handled = yield* conversation.ReadCursor(thread_id).pipe(
							Effect.flatMap((head) => {
								if (
									head.status !== "available" ||
									head.conversation_id !== cursor.conversation_id ||
									cursor.last_patch_sequence > head.last_patch_sequence
								)
									return Effect.succeed(false);
								const stream_id = `projection:conversation:${thread_id}:${subscription_id}`;
								const subscription: ConversationProjectionSubscription = {
									_tag: "conversation",
									thread_id,
									journal_sequence: current.delivered_journal_sequence,
									patch_sequence: cursor.last_patch_sequence,
									sequence: -1,
									stream_id,
								};
								return runtime
									.StartWithoutSnapshot(
										correlation_id,
										subscription_id,
										claim,
										current,
										subscription,
									)
									.pipe(
										Effect.flatMap((registered) =>
											EnqueuePatches(registered).pipe(
												Effect.flatMap((subscriptions) =>
													Ref.modify(state, (latest) =>
														latest._tag !== "Ready"
															? ([undefined, latest] as const)
															: ([
																	undefined,
																	{
																		...latest,
																		subscriptions:
																			MergeOwnedSubscriptionUpdates(
																				latest,
																				registered.subscriptions,
																				subscriptions,
																			),
																	},
																] as const),
													),
												),
											),
										),
										Effect.as(true),
									);
							}),
							Effect.catch(() =>
								EnqueueProjectionError(
									"The conversation projection could not be read.",
								).pipe(Effect.as(true)),
							),
						);
						if (cursor_handled) return;
					}
					yield* conversation.ReadSnapshot(thread_id).pipe(
						Effect.flatMap((availability) => {
							if (availability.status !== "available")
								return EnqueueProjectionError(
									"The conversation projection is unavailable.",
								);

							const snapshot = availability.snapshot;
							const stream_id = `projection:conversation:${thread_id}:${subscription_id}`;
							const subscription: ConversationProjectionSubscription = {
								_tag: "conversation",
								thread_id,
								journal_sequence: snapshot.journal_sequence,
								patch_sequence: snapshot.last_patch_sequence,
								sequence: 0,
								stream_id,
							};
							return runtime
								.Start(
									correlation_id,
									subscription_id,
									claim,
									current,
									subscription,
									({ message_id, sent_at }) => ({
										journal_sequence: snapshot.journal_sequence,
										kind: "conversation.snapshot",
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
								)
								.pipe(
									Effect.flatMap((registered) =>
										EnqueuePatches(registered).pipe(
											Effect.flatMap((subscriptions) =>
												Ref.modify(state, (latest) =>
													latest._tag !== "Ready"
														? ([undefined, latest] as const)
														: ([
																undefined,
																{
																	...latest,
																	subscriptions:
																		MergeOwnedSubscriptionUpdates(
																			latest,
																			registered.subscriptions,
																			subscriptions,
																		),
																},
															] as const),
												),
											),
										),
									),
								);
						}),
						Effect.catch(() =>
							EnqueueProjectionError(
								"The conversation projection could not be read.",
							),
						),
					);
					return;
				}

				yield* transcript.Read({ thread_id }).pipe(
					Effect.flatMap((snapshot) => {
						const stream_id = `projection:thread.transcript:${thread_id}:${subscription_id}`;
						const subscription: ThreadTranscriptProjectionSubscription = {
							_tag: "thread.transcript",
							thread_id,
							journal_sequence: snapshot.journal_sequence,
							sequence: 0,
							stream_id,
						};
						return runtime.Start(
							correlation_id,
							subscription_id,
							claim,
							current,
							subscription,
							({ message_id, sent_at }) => ({
								journal_sequence: snapshot.journal_sequence,
								kind: "thread.transcript.snapshot",
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
					}),
					Effect.catch(() =>
						EnqueueProjectionError("The thread transcript could not be read."),
					),
				);
			}),
		);

	return { Handle };
});
