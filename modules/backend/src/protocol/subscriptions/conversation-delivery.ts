import { Context, Effect } from "effect";

import {
	ConversationReadModel,
	conversation_patch_replay_batch_size,
	type ConversationReadModelFailure,
} from "../../conversation";
import { RuntimeMetadata } from "../../runtime/metadata";
import type { ProjectionSubscription, ReadyState } from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";

export class ConnectionConversationDelivery extends Context.Service<
	ConnectionConversationDelivery,
	{
		readonly EnqueuePatches: (
			current: ReadyState,
		) => Effect.Effect<
			Readonly<Record<string, ProjectionSubscription>>,
			ConversationReadModelFailure
		>;
	}
>()("Artisan/ConnectionConversationDelivery") {}

export const MakeConnectionConversationDelivery = Effect.gen(function* () {
	const conversation = yield* ConversationReadModel;
	const metadata = yield* RuntimeMetadata;
	const { Enqueue } = yield* ConnectionSubscriptionControl;

	const EnqueuePatches = (current: ReadyState) =>
		Effect.gen(function* () {
			const maximum_batches_per_delivery = 4;
			let subscriptions = current.subscriptions;

			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
				if (subscription._tag !== "conversation") continue;
				let patch_sequence = subscription.patch_sequence;
				let stream_sequence = subscription.sequence;
				let delivered_batches = 0;

				while (delivered_batches < maximum_batches_per_delivery) {
					const patches = yield* conversation.ReadPatches(
						subscription.thread_id,
						patch_sequence,
					);
					if (patches.length === 0) break;
					const final_patch = patches[patches.length - 1];
					if (final_patch === undefined) break;

					delivered_batches += 1;
					stream_sequence += 1;
					const from_sequence = patch_sequence + 1;
					patch_sequence = final_patch.sequence;
					yield* Enqueue({
						journal_sequence: current.delivered_journal_sequence,
						kind: "conversation.patch",
						message_id: yield* metadata.MakeId("message"),
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
						sent_at: yield* metadata.Now,
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

	return ConnectionConversationDelivery.of({ EnqueuePatches });
});
