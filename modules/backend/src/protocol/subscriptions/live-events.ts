import { Cause, Chunk, Effect, Option, Ref } from "effect";
import type { EventEnvelope, OutboundControlEnvelope } from "@artisan/protocol";
import { RuntimeMetadata } from "../../runtime/metadata";
import { SurfaceService } from "../../surfaces/service";
import {
	ApplyEventCursors,
	LatestJournalSequence,
	MergeOwnedSubscriptionUpdates,
	type PendingSubscription,
	type ProjectionSubscription,
	type ReadyState,
} from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";
import { ConnectionConversationDelivery } from "./conversation-delivery";
import { EventAffectsConversation } from "./patch-selection";
import { MakeConnectionProjectionPatches } from "./projection-patches";

/** Constructs ordered live-event and projection delivery for one connection. */
export const MakeLiveEventDelivery = Effect.gen(function* () {
	const {
		state,
		Enqueue: EnqueueUnsafe,
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
	const PublishProjection = (
		current: ReadyState,
		subscription_id: string,
		publication: Effect.Effect<void>,
	) => {
		const claim = current.subscription_claims?.[subscription_id];
		return claim === undefined
			? Effect.succeed(Option.none<void>())
			: PublishClaim(subscription_id, claim, publication);
	};
	const conversation_delivery = yield* ConnectionConversationDelivery;
	const surfaces = yield* SurfaceService;
	const metadata = yield* RuntimeMetadata;
	const { EnqueueProjectionPatches } = yield* MakeConnectionProjectionPatches;
	/** `0` notifier wakes have no durable event. Refresh only projections that
	 * observation persistence wrote, sharing each equivalent query once. */
	const EnqueueProjectionOnlySurfacePatches = (current: ReadyState) =>
		Effect.gen(function* () {
			const Enqueue = (envelope: OutboundControlEnvelope) =>
				PublishProjection(
					current,
					"subscription_id" in envelope ? envelope.subscription_id : "",
					EnqueueUnsafe(envelope),
				).pipe(Effect.asVoid);
			const cached_reads = new Map<string, Effect.Effect<unknown, unknown>>();
			const CachedRead = <A, E>(key: string, read: Effect.Effect<A, E>) =>
				Effect.gen(function* () {
					const existing = cached_reads.get(key) as Effect.Effect<A, E> | undefined;
					if (existing !== undefined) return yield* existing;
					const cached = yield* Effect.cached(read);
					cached_reads.set(key, cached);
					return yield* cached;
				});
			let subscriptions = current.subscriptions;
			const sent_at = yield* metadata.Now;
			for (const [subscription_id, subscription] of Object.entries(current.subscriptions)) {
				if (subscription._tag === "surface.list") {
					const key = JSON.stringify([
						"surface",
						subscription.query.thread_id,
						subscription.query.run_id,
						subscription.query.group_id,
					]);
					const snapshot = yield* CachedRead(
						key,
						surfaces.ListSnapshot({
							thread_id: subscription.query.thread_id,
							...(subscription.query.run_id === undefined
								? {}
								: { run_id: subscription.query.run_id }),
							...(subscription.query.group_id === undefined
								? {}
								: { group_id: subscription.query.group_id }),
						}),
					);
					const sequence = subscription.sequence + 1;
					yield* Enqueue({
						journal_sequence: snapshot.journal_sequence,
						kind: "surface.list.snapshot",
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
						[subscription_id]: {
							...subscription,
							journal_sequence: snapshot.journal_sequence,
							sequence,
						},
					};
				} else if (subscription._tag === "surface.usage.aggregate") {
					const key = JSON.stringify([
						"usage",
						subscription.query.scope,
						subscription.query.scope_id,
					]);
					const snapshot = yield* CachedRead(
						key,
						surfaces.AggregateUsageSnapshot(subscription.query),
					);
					const thread_id = yield* CachedRead(
						`${key}:thread`,
						surfaces.UsageScopeThread(subscription.query),
					);
					const sequence = subscription.sequence + 1;
					yield* Enqueue({
						journal_sequence: snapshot.journal_sequence,
						kind: "surface.usage.aggregate.snapshot",
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
					const { thread_id: _previous_thread_id, ...without_thread_id } = subscription;
					subscriptions = {
						...subscriptions,
						[subscription_id]: {
							...without_thread_id,
							...(thread_id === undefined ? {} : { thread_id }),
							journal_sequence: snapshot.journal_sequence,
							sequence,
						},
					};
				}
			}
			return subscriptions;
		});

	/**
	 * Admission advances `delivered_journal_sequence` before the projection
	 * phases run, so a phase failure can never be replayed by a later wake —
	 * the admitted events would simply lose their projection deliveries and,
	 * at the end of a run, freeze the thread with nothing owed to heal it.
	 * Every phase therefore degrades to the subscriptions it had, keeps the
	 * remaining phases running, and lets cursor-based redelivery repair the
	 * failed one on the next event.
	 */
	const IsolateDeliveryPhase =
		(phase: string, fallback: Readonly<Record<string, ProjectionSubscription>>) =>
		<E, R>(work: Effect.Effect<Readonly<Record<string, ProjectionSubscription>>, E, R>) =>
			work.pipe(
				Effect.matchCauseEffect({
					onFailure: (cause) =>
						Cause.hasInterruptsOnly(cause)
							? Effect.failCause(cause)
							: Effect.logWarning(
									"Live delivery phase failed; remaining phases continue",
									{ cause, phase },
								).pipe(Effect.as(fallback)),
					onSuccess: Effect.succeed,
				}),
			);

	const DeliverLiveEvents = (
		events: ReadonlyArray<EventEnvelope>,
		options: { readonly projection_only?: boolean } = {},
	) =>
		Effect.gen(function* () {
			/* A positive notifier with an empty trusted tail is a duplicate wake.
			 * Only notifier zero explicitly requests a projection-only refresh. */
			if (events.length === 0 && !options.projection_only) return;

			/**
			 * The projection phase must observe the state as it stood under the
			 * delivery-admission lock, never an earlier read: subscription
			 * registration also runs under that lock, so a pre-admission read can
			 * predate a subscription for which this delivery is the only wake.
			 * A subscription registered after this admission reads its snapshot
			 * after these events committed, so it already contains them.
			 */
			const admission = yield* WithDeliveryAdmission(
				Effect.gen(function* () {
					const admitted = yield* Ref.get(state);
					if (admitted._tag !== "Ready") return undefined;
					const new_events = events.filter(
						(event) => event.journal_sequence > admitted.delivered_journal_sequence,
					);
					yield* Effect.forEach(new_events, EnqueueUnsafe, { discard: true });
					yield* Ref.modify(state, (latest) => {
						if (latest._tag !== "Ready") return [undefined, latest] as const;
						return [
							undefined,
							{
								...latest,
								delivered_cursors: ApplyEventCursors(
									latest.delivered_cursors,
									new_events,
								),
								delivered_journal_sequence: LatestJournalSequence(
									latest.delivered_journal_sequence,
									new_events,
								),
								unacknowledged_events: Chunk.appendAll(
									latest.unacknowledged_events ?? Chunk.empty(),
									Chunk.fromIterable(new_events),
								),
							},
						] as const;
					});
					return { admitted, new_events };
				}),
			);
			if (admission === undefined) return;
			const { admitted: current, new_events } = admission;

			let subscriptions = current.subscriptions;
			for (const event of new_events) {
				subscriptions = yield* EnqueueProjectionPatches(
					{ ...current, subscriptions },
					event,
				).pipe(IsolateDeliveryPhase("event_projections", subscriptions));
			}

			const delivered_journal_sequence = LatestJournalSequence(
				current.delivered_journal_sequence,
				new_events,
			);
			const affected_conversation_threads = new Set(
				new_events.filter(EventAffectsConversation).map((event) => event.thread_id),
			);
			subscriptions = yield* conversation_delivery
				.EnqueuePatches(
					{
						...current,
						delivered_journal_sequence,
						subscriptions,
					},
					new_events.length === 0 ? undefined : affected_conversation_threads,
				)
				.pipe(IsolateDeliveryPhase("conversation_patches", subscriptions));
			if (options.projection_only) {
				subscriptions = yield* EnqueueProjectionOnlySurfacePatches({
					...current,
					delivered_journal_sequence,
					subscriptions,
				}).pipe(IsolateDeliveryPhase("projection_only_surfaces", subscriptions));
			}
			yield* Ref.modify(state, (latest) => {
				if (latest._tag !== "Ready") return [undefined, latest] as const;
				return [
					undefined,
					{
						...latest,
						subscriptions: MergeOwnedSubscriptionUpdates(
							latest,
							current.subscriptions,
							subscriptions,
						),
					},
				] as const;
			});
		});

	return { DeliverLiveEvents };
});
