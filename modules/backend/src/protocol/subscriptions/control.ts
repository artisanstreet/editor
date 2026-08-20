import { Chunk, Context, Deferred, Effect, Option, Ref, Semaphore } from "effect";

import {
	type AckEnvelope,
	type EventEnvelope,
	type HeartbeatPongEnvelope,
	type OutboundControlEnvelope,
	type ReplayEnvelope,
} from "@artisan/protocol";

import { JournalStore } from "../../persistence/journal-store";
import { RuntimeMetadata } from "../../runtime/metadata";
import {
	CursorsAtAcknowledgement,
	type ConnectionState,
	CursorsToRecord,
	type PendingSubscription,
	type ReadyState,
	RecordToCursors,
	SameCursors,
} from "../connection-state";

export class ConnectionSubscriptionControl extends Context.Service<
	ConnectionSubscriptionControl,
	{
		readonly state: Ref.Ref<ConnectionState>;
		readonly RequestClose?: Effect.Effect<void>;
		readonly WithDeliveryAdmission?: <A, E, R>(
			effect: Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E, R>;
		readonly Enqueue: (envelope: OutboundControlEnvelope) => Effect.Effect<void>;
		readonly EnqueueError: (
			current: ConnectionState,
			code: string,
			message: string,
			retryable: boolean,
			correlation_id?: string,
		) => Effect.Effect<void>;
		readonly ClaimPending?: (
			subscription_id: string,
			message_id: string,
		) => Effect.Effect<SubscriptionClaimResult>;
		readonly ClaimCancellation?: (
			subscription_id: string,
		) => Effect.Effect<PendingSubscription | undefined>;
		readonly CancelPending?: (
			subscription_id: string,
			claim?: PendingSubscription,
		) => Effect.Effect<boolean>;
		readonly FinalizePending?: (
			subscription_id: string,
			claim: PendingSubscription,
		) => Effect.Effect<void>;
		readonly PublishClaim?: <A, E, R>(
			subscription_id: string,
			claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => Effect.Effect<Option.Option<A>, E, R>;
		readonly PublishTerminal?: <A, E, R>(
			subscription_id: string,
			claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => Effect.Effect<Option.Option<A>, E, R>;
	}
>()("Artisan/ConnectionSubscriptionControl") {}

type AckAdvanceResult =
	| { readonly _tag: "accepted" }
	| { readonly _tag: "closed" }
	| { readonly _tag: "invalid"; readonly latest: ReadyState };

export type SubscriptionClaimResult =
	| PendingSubscription
	| { readonly _tag: "await_stopping"; readonly stopping: PendingSubscription }
	| undefined;

const OwnsSubscriptionClaim = (
	latest: ReadyState,
	subscription_id: string,
	claim: PendingSubscription,
) =>
	latest.pending_subscriptions?.[subscription_id] === claim ||
	latest.subscription_claims?.[subscription_id] === claim ||
	latest.stopping_subscriptions?.[subscription_id] === claim;

/** Builds one connection's exact-generation claim and publication lifecycle. */
export const MakeSubscriptionClaimLifecycle = (state: Ref.Ref<ConnectionState>) => {
	const ClaimPending = (subscription_id: string, message_id: string) =>
		Effect.gen(function* () {
			const cancelled = yield* Deferred.make<void>();
			const released = yield* Deferred.make<void>();
			const publication = yield* Semaphore.make(1);
			const candidate: PendingSubscription = { cancelled, message_id, publication, released };
			return yield* Ref.modify(
				state,
				(latest): readonly [SubscriptionClaimResult, ConnectionState] => {
					if (latest._tag !== "Ready") return [undefined, latest] as const;
					const stopping = latest.stopping_subscriptions?.[subscription_id];
					if (stopping !== undefined)
						return [{ _tag: "await_stopping", stopping } as const, latest] as const;
					if (
						latest.subscriptions[subscription_id] !== undefined ||
						latest.pending_subscriptions?.[subscription_id] !== undefined ||
						latest.subscription_claims?.[subscription_id] !== undefined
					)
						return [undefined, latest] as const;
					return [
						candidate,
						{
							...latest,
							pending_subscriptions: {
								...latest.pending_subscriptions,
								[subscription_id]: candidate,
							},
						},
					] as const;
				},
			);
		});

	const FinalizePending = (subscription_id: string, claim: PendingSubscription) =>
		Ref.modify(state, (latest) => {
			if (
				latest._tag !== "Ready" ||
				latest.pending_subscriptions?.[subscription_id] !== claim
			)
				return [undefined, latest] as const;
			const { [subscription_id]: _removed, ...pending_subscriptions } =
				latest.pending_subscriptions ?? {};
			return [undefined, { ...latest, pending_subscriptions }] as const;
		});

	const ReleaseClaim = (subscription_id: string, claim: PendingSubscription) =>
		Ref.modify(state, (latest) => {
			if (latest._tag !== "Ready" || !OwnsSubscriptionClaim(latest, subscription_id, claim))
				return [undefined, latest] as const;
			const { [subscription_id]: _claim_removed, ...subscription_claims } =
				latest.subscription_claims ?? {};
			const { [subscription_id]: _stopping_removed, ...stopping_subscriptions } =
				latest.stopping_subscriptions ?? {};
			return [claim, { ...latest, stopping_subscriptions, subscription_claims }] as const;
		}).pipe(
			Effect.tap((released) =>
				released === undefined
					? Effect.void
					: Deferred.succeed(released.released, undefined),
			),
			Effect.asVoid,
		);

	const PublishClaim = <A, E, R>(
		subscription_id: string,
		claim: PendingSubscription,
		publication: Effect.Effect<A, E, R>,
	) =>
		Semaphore.withPermit(claim.publication)(
			Ref.get(state).pipe(
				Effect.flatMap((latest) =>
					latest._tag === "Ready" && OwnsSubscriptionClaim(latest, subscription_id, claim)
						? publication.pipe(Effect.map(Option.some))
						: Effect.succeed(Option.none<A>()),
				),
			),
		);

	const ClaimCancellation = (subscription_id: string, expected_claim?: PendingSubscription) =>
		Effect.gen(function* () {
			const observed = yield* Ref.get(state);
			if (observed._tag !== "Ready") return undefined;
			const claim =
				expected_claim ??
				observed.pending_subscriptions?.[subscription_id] ??
				(observed.subscriptions[subscription_id] === undefined
					? undefined
					: observed.subscription_claims?.[subscription_id]);
			if (claim === undefined) return undefined;

			return yield* Ref.modify(state, (latest) => {
				if (
					latest._tag !== "Ready" ||
					!OwnsSubscriptionClaim(latest, subscription_id, claim)
				)
					return [undefined, latest] as const;
				if (latest.stopping_subscriptions?.[subscription_id] === claim)
					return [undefined, latest] as const;
				const { [subscription_id]: _pending_removed, ...pending_subscriptions } =
					latest.pending_subscriptions ?? {};
				const { [subscription_id]: _subscription_removed, ...subscriptions } =
					latest.subscriptions;
				return [
					claim,
					{
						...latest,
						pending_subscriptions,
						stopping_subscriptions: {
							...latest.stopping_subscriptions,
							[subscription_id]: claim,
						},
						subscriptions,
					},
				] as const;
			}).pipe(
				Effect.tap((cancelled) =>
					cancelled === undefined
						? Effect.void
						: Deferred.succeed(claim.cancelled, undefined),
				),
			);
		});

	const PublishTerminal = <A, E, R>(
		subscription_id: string,
		claim: PendingSubscription,
		publication: Effect.Effect<A, E, R>,
	) =>
		Semaphore.withPermit(claim.publication)(
			Effect.gen(function* () {
				const admitted = yield* Ref.modify(state, (latest) => {
					if (
						latest._tag !== "Ready" ||
						!OwnsSubscriptionClaim(latest, subscription_id, claim)
					)
						return [false, latest] as const;
					const { [subscription_id]: _pending_removed, ...pending_subscriptions } =
						latest.pending_subscriptions ?? {};
					const { [subscription_id]: _subscription_removed, ...subscriptions } =
						latest.subscriptions;
					return [
						true,
						{
							...latest,
							pending_subscriptions,
							stopping_subscriptions: {
								...latest.stopping_subscriptions,
								[subscription_id]: claim,
							},
							subscriptions,
						},
					] as const;
				});
				if (!admitted) return Option.none<A>();
				yield* Deferred.succeed(claim.cancelled, undefined);
				return yield* publication.pipe(
					Effect.map(Option.some),
					Effect.ensuring(ReleaseClaim(subscription_id, claim)),
				);
			}),
		);

	/** Compatibility helper for isolated lifecycle tests; protocol delivery uses ClaimCancellation. */
	const CancelPending = (subscription_id: string, expected_claim?: PendingSubscription) =>
		ClaimCancellation(subscription_id, expected_claim).pipe(
			Effect.tap((claim) =>
				claim === undefined ? Effect.void : ReleaseClaim(subscription_id, claim),
			),
			Effect.map((claim) => claim !== undefined),
		);

	return Effect.succeed({
		CancelPending,
		ClaimCancellation,
		ClaimPending,
		FinalizePending,
		PublishClaim,
		PublishTerminal,
	} as const);
};

export const MakeSubscriptionControlHandlers = Effect.gen(function* () {
	const journal = yield* JournalStore;
	const metadata = yield* RuntimeMetadata;
	const control = yield* ConnectionSubscriptionControl;
	const {
		state,
		Enqueue,
		EnqueueError,
		WithDeliveryAdmission: WithDeliveryAdmissionOption,
	} = control;
	const WithDeliveryAdmission = WithDeliveryAdmissionOption ?? ((effect) => effect);
	const lifecycle =
		control.ClaimCancellation !== undefined &&
		control.ClaimPending !== undefined &&
		control.FinalizePending !== undefined &&
		control.PublishClaim !== undefined &&
		control.PublishTerminal !== undefined
			? {
					CancelPending: control.CancelPending ?? (() => Effect.succeed(false)),
					ClaimCancellation: control.ClaimCancellation,
					ClaimPending: control.ClaimPending,
					FinalizePending: control.FinalizePending,
					PublishClaim: control.PublishClaim,
					PublishTerminal: control.PublishTerminal,
				}
			: yield* MakeSubscriptionClaimLifecycle(state);
	const {
		CancelPending,
		ClaimCancellation,
		ClaimPending,
		FinalizePending,
		PublishClaim,
		PublishTerminal,
	} = lifecycle;
	const EnqueueReplayEvents = (events: ReadonlyArray<EventEnvelope>) =>
		WithDeliveryAdmission(
			Effect.gen(function* () {
				const current = yield* Ref.get(state);
				if (current._tag !== "Ready") return false;
				const new_events = events.filter(
					(event) => event.journal_sequence > current.delivered_journal_sequence,
				);
				yield* Effect.forEach(new_events, Enqueue, { discard: true });
				yield* Ref.modify(state, (latest) => {
					if (latest._tag !== "Ready") return [undefined, latest] as const;
					return [
						undefined,
						{
							...latest,
							delivered_cursors: new_events.reduce<Readonly<Record<string, number>>>(
								(cursors, event) => ({
									...cursors,
									[event.stream_id]: Math.max(
										cursors[event.stream_id] ?? 0,
										event.sequence,
									),
								}),
								latest.delivered_cursors,
							),
							delivered_journal_sequence: new_events.reduce(
								(sequence, event) => Math.max(sequence, event.journal_sequence),
								latest.delivered_journal_sequence,
							),
							unacknowledged_events: Chunk.appendAll(
								latest.unacknowledged_events ?? Chunk.empty(),
								Chunk.fromIterable(new_events),
							),
						},
					] as const;
				});
				return true;
			}),
		);

	const HandleAck = (ack: AckEnvelope, _current: ReadyState) =>
		Effect.gen(function* () {
			const result = yield* WithDeliveryAdmission(
				Ref.modify(state, (latest): readonly [AckAdvanceResult, ConnectionState] => {
					if (latest._tag !== "Ready")
						return [{ _tag: "closed" } as const, latest] as const;

					const invalid_journal =
						ack.payload.journal_sequence < latest.acknowledged_journal_sequence ||
						ack.payload.journal_sequence > latest.delivered_journal_sequence ||
						(ack.payload.journal_sequence !== latest.acknowledged_journal_sequence &&
							!Chunk.some(
								latest.unacknowledged_events ?? Chunk.empty(),
								(event) => event.journal_sequence === ack.payload.journal_sequence,
							));
					const acknowledged_cursors = CursorsToRecord(ack.payload.event_cursors);
					const duplicate_cursor =
						Object.keys(acknowledged_cursors).length !==
						ack.payload.event_cursors.length;
					const expected_cursors = CursorsAtAcknowledgement(
						latest.acknowledged_cursors,
						latest.unacknowledged_events ?? Chunk.empty(),
						ack.payload.journal_sequence,
					);
					const invalid_stream =
						duplicate_cursor || !SameCursors(expected_cursors, acknowledged_cursors);

					if (invalid_journal || invalid_stream) {
						return [{ _tag: "invalid", latest } as const, latest] as const;
					}

					return [
						{ _tag: "accepted" } as const,
						{
							...latest,
							acknowledged_cursors,
							acknowledged_journal_sequence: ack.payload.journal_sequence,
							unacknowledged_events: Chunk.filter(
								latest.unacknowledged_events ?? Chunk.empty(),
								(event) => event.journal_sequence > ack.payload.journal_sequence,
							),
						},
					] as const;
				}),
			);

			if (result._tag === "invalid") {
				yield* EnqueueError(
					result.latest,
					"protocol.invalid_ack",
					"The acknowledgement is outside the delivered range.",
					false,
					ack.message_id,
				);
			}
		});

	const HandleReplay = (replay: ReplayEnvelope, current: ReadyState) =>
		journal
			.ReadReplay({
				after_journal_sequence: replay.payload.after_journal_sequence,
				...(replay.payload.event_cursors
					? { stream_cursors: replay.payload.event_cursors }
					: {}),
			})
			.pipe(
				Effect.flatMap((events) =>
					Effect.gen(function* () {
						const message_id = yield* metadata.MakeId("message");
						const sent_at = yield* metadata.Now;

						const admitted = yield* EnqueueReplayEvents(events);
						if (!admitted) return;
						const updated = yield* Ref.get(state);
						if (updated._tag !== "Ready") return;

						yield* Enqueue({
							correlation_id: replay.message_id,
							kind: "replay.complete",
							message_id,
							origin: "backend",
							payload: {
								current_event_cursors: RecordToCursors(updated.delivered_cursors),
								journal_sequence: updated.delivered_journal_sequence,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
					}),
				),
				Effect.catch(() =>
					EnqueueError(
						current,
						"protocol.replay_invalid",
						"The replay cursor does not match the journal.",
						false,
						replay.message_id,
					),
				),
			);

	const HandlePong = (pong: HeartbeatPongEnvelope, _current: ReadyState) =>
		Effect.gen(function* () {
			const result = yield* Ref.modify(
				state,
				(latest): readonly [AckAdvanceResult, ConnectionState] => {
					if (latest._tag !== "Ready")
						return [{ _tag: "closed" } as const, latest] as const;
					const pending = latest.pending_heartbeat;
					const matches =
						pending?.message_id === pong.correlation_id &&
						pending.nonce === pong.payload.nonce;
					if (!matches) return [{ _tag: "invalid", latest } as const, latest] as const;
					const { pending_heartbeat: _, ...without_pending } = latest;
					return [{ _tag: "accepted" } as const, without_pending] as const;
				},
			);

			if (result._tag === "invalid") {
				yield* EnqueueError(
					result.latest,
					"protocol.invalid_heartbeat",
					"The heartbeat response does not match an active ping.",
					false,
					pong.message_id,
				);
			}
		});

	return {
		CancelPending,
		ClaimPending,
		ClaimCancellation,
		FinalizePending,
		HandleAck,
		HandlePong,
		HandleReplay,
		PublishClaim,
		PublishTerminal,
	};
});

export type SubscriptionControlHandlers = Effect.Success<typeof MakeSubscriptionControlHandlers>;
