import { Effect, Ref } from "effect";

import type { SubscribeEnvelope } from "@artisan/protocol";

import { ConversationReadModel } from "../../conversation/index.ts";
import { TranscriptReadModel } from "../../persistence/transcript-read-model";
import type {
	ConversationProjectionSubscription,
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
	const { state, EnqueueError } = yield* ConnectionSubscriptionControl;

	const Handle = (subscribe: SubscribeEnvelope, current: ReadyState) =>
		Effect.gen(function* () {
			if (
				subscribe.payload.type !== "conversation" &&
				subscribe.payload.type !== "thread.transcript"
			)
				return;
			const subscription_id = subscribe.subscription_id;
			const correlation_id = subscribe.message_id;
			const thread_id = subscribe.payload.thread_id;

			if (subscribe.payload.type === "conversation") {
				yield* conversation.ReadSnapshot(thread_id).pipe(
					Effect.flatMap((availability) => {
						if (availability.status !== "available")
							return EnqueueError(
								current,
								"projection.unavailable",
								"The conversation projection is unavailable.",
								true,
								correlation_id,
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
											Ref.set(state, { ...registered, subscriptions }),
										),
									),
								),
							);
					}),
					Effect.catch(() =>
						EnqueueError(
							current,
							"projection.unavailable",
							"The conversation projection could not be read.",
							true,
							correlation_id,
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
					EnqueueError(
						current,
						"projection.unavailable",
						"The thread transcript could not be read.",
						true,
						correlation_id,
					),
				),
			);
		});

	return { Handle };
});
