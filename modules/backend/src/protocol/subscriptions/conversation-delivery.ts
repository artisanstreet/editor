import { Context, Effect, Option } from "effect";
import type { ConversationPatch } from "@artisan/protocol";

import {
	ConversationReadModel,
	conversation_patch_replay_batch_size,
	conversation_patch_retention_limit,
	type ConversationAvailability,
	type ConversationReadModelFailure,
} from "../../conversation";
import { RuntimeMetadata } from "../../runtime/metadata";
import type { PendingSubscription, ProjectionSubscription, ReadyState } from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";

export class ConnectionConversationDelivery extends Context.Service<
	ConnectionConversationDelivery,
	{
		readonly EnqueuePatches: (
			current: ReadyState,
			affected_thread_ids?: ReadonlySet<string>,
		) => Effect.Effect<
			Readonly<Record<string, ProjectionSubscription>>,
			ConversationReadModelFailure
		>;
	}
>()("Artisan/ConnectionConversationDelivery") {}

export const MakeConnectionConversationDelivery = Effect.gen(function* () {
	const conversation = yield* ConversationReadModel;
	const metadata = yield* RuntimeMetadata;
	const { Enqueue, PublishClaim: PublishClaimOption } = yield* ConnectionSubscriptionControl;
	const PublishClaim =
		PublishClaimOption ??
		(<A, E, R>(
			_subscription_id: string,
			_claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => publication.pipe(Effect.map(Option.some)));

	const EnqueuePatches = (current: ReadyState, affected_thread_ids?: ReadonlySet<string>) =>
		Effect.gen(function* () {
			/**
			 * One delivery must be able to drain the entire retained patch window:
			 * a subscription behind by more than this budget but still inside
			 * retention would stop mid-window with no later wake owed to it, while
			 * anything beyond retention already heals through the snapshot fallback.
			 */
			const maximum_batches_per_delivery =
				conversation_patch_retention_limit / conversation_patch_replay_batch_size;
			/**
			 * Several renderer surfaces can hold an equivalent conversation cursor
			 * during navigation. A delivery observes one authoritative projection
			 * point, so share its reads only for this invocation: the next delivery
			 * must always consult fresh conversation state.
			 */
			const cached_patch_reads = new Map<
				string,
				Effect.Effect<ReadonlyArray<ConversationPatch>, ConversationReadModelFailure>
			>();
			const cached_snapshot_reads = new Map<
				string,
				Effect.Effect<ConversationAvailability, ConversationReadModelFailure>
			>();
			const ReadPatches = (thread_id: string, patch_sequence: number) =>
				Effect.gen(function* () {
					const key = JSON.stringify(["patch", thread_id, patch_sequence]);
					const existing = cached_patch_reads.get(key);
					if (existing !== undefined) return yield* existing;
					const cached = yield* Effect.cached(
						conversation.ReadPatches(thread_id, patch_sequence),
					);
					cached_patch_reads.set(key, cached);
					return yield* cached;
				});
			const ReadSnapshot = (thread_id: string) =>
				Effect.gen(function* () {
					const key = JSON.stringify(["snapshot", thread_id]);
					const existing = cached_snapshot_reads.get(key);
					if (existing !== undefined) return yield* existing;
					const cached = yield* Effect.cached(conversation.ReadSnapshot(thread_id));
					cached_snapshot_reads.set(key, cached);
					return yield* cached;
				});
			let subscriptions = current.subscriptions;

			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
				if (subscription._tag !== "conversation") continue;
				const claim = current.subscription_claims?.[subscription_id];
				if (claim === undefined) continue;
				if (
					affected_thread_ids !== undefined &&
					!affected_thread_ids.has(subscription.thread_id)
				)
					continue;
				let patch_sequence = subscription.patch_sequence;
				let stream_sequence = subscription.sequence;
				let delivered_batches = 0;

				while (delivered_batches < maximum_batches_per_delivery) {
					const patches = yield* ReadPatches(subscription.thread_id, patch_sequence);
					if (patches.length === 0) break;
					const final_patch = patches[patches.length - 1];
					if (final_patch === undefined) break;
					const first_patch = patches[0];
					if (first_patch === undefined) break;
					if (first_patch.sequence !== patch_sequence + 1) {
						const availability = yield* ReadSnapshot(subscription.thread_id);
						if (availability.status !== "available") break;
						const snapshot = availability.snapshot;
						stream_sequence += 1;
						yield* PublishClaim(
							subscription_id,
							claim,
							Enqueue({
								journal_sequence: snapshot.journal_sequence,
								kind: "conversation.snapshot",
								message_id: yield* metadata.MakeId("message"),
								origin: "backend",
								payload: snapshot,
								protocol_version: 1,
								schema_version: 1,
								sent_at: yield* metadata.Now,
								sequence: stream_sequence,
								stream_id: subscription.stream_id,
								subscription_id,
							}),
						);
						patch_sequence = snapshot.last_patch_sequence;
						subscriptions = {
							...subscriptions,
							[subscription_id]: {
								...subscription,
								journal_sequence: snapshot.journal_sequence,
								patch_sequence,
								sequence: stream_sequence,
							},
						};
						break;
					}

					delivered_batches += 1;
					stream_sequence += 1;
					const from_sequence = patch_sequence + 1;
					patch_sequence = final_patch.sequence;
					yield* PublishClaim(
						subscription_id,
						claim,
						Enqueue({
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
						}),
					);
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
