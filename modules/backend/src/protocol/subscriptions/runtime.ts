import { Context, Effect, Option, Ref } from "effect";

import type { OutboundControlEnvelope } from "@artisan/protocol";

import { RuntimeMetadata } from "../../runtime/metadata";
import type {
	ConnectionState,
	PendingSubscription,
	ProjectionSubscription,
	ReadyState,
} from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";

interface ProjectionMetadata {
	readonly message_id: string;
	readonly sent_at: string;
}

type RegistrationAdmission =
	| { readonly admitted: false }
	| { readonly admitted: true; readonly registered: ReadyState };

export class ConnectionProjectionRuntime extends Context.Service<
	ConnectionProjectionRuntime,
	{
		readonly Start: (
			correlation_id: string,
			subscription_id: string,
			claim: PendingSubscription,
			current: ReadyState,
			subscription: ProjectionSubscription,
			MakeSnapshot: (metadata: ProjectionMetadata) => OutboundControlEnvelope,
		) => Effect.Effect<ReadyState>;
		readonly StartWithoutSnapshot: (
			correlation_id: string,
			subscription_id: string,
			claim: PendingSubscription,
			current: ReadyState,
			subscription: ProjectionSubscription,
		) => Effect.Effect<ReadyState>;
	}
>()("Artisan/ConnectionProjectionRuntime") {}

export const MakeConnectionProjectionRuntime = Effect.gen(function* () {
	const metadata = yield* RuntimeMetadata;
	const {
		state,
		Enqueue,
		EnqueueError,
		PublishClaim: PublishClaimOption,
	} = yield* ConnectionSubscriptionControl;
	const PublishClaim =
		PublishClaimOption ??
		(<A, E, R>(
			_subscription_id: string,
			_claim: PendingSubscription,
			publication: Effect.Effect<A, E, R>,
		) => publication.pipe(Effect.map(Option.some)));

	/**
	 * Answers a subscribe that registration refused.
	 *
	 * Registration is refused when the connection has left `Ready` or when the
	 * pending claim was superseded between the subscribe arriving and this
	 * running. Both are ordinary races, but a subscriber has no way to observe
	 * either: it is parked on the `subscription.started` this would otherwise
	 * never send, and a park is not a failure, so no retry schedule can rescue
	 * it. That is how a thread list stayed empty until the window was reloaded.
	 *
	 * The refusal is retryable because nothing about the request was wrong —
	 * the correlation is the subscribe's own message id, so it fails exactly
	 * the one waiter and the client's existing retry re-subscribes.
	 */
	const RefuseRegistration = (correlation_id: string, current: ReadyState) =>
		EnqueueError(
			current,
			"subscription.unavailable",
			"The subscription could not be registered on this connection.",
			true,
			correlation_id,
		);

	const Start = (
		correlation_id: string,
		subscription_id: string,
		claim: PendingSubscription,
		current: ReadyState,
		subscription: ProjectionSubscription,
		MakeSnapshot: (metadata: ProjectionMetadata) => OutboundControlEnvelope,
	) =>
		PublishClaim(
			subscription_id,
			claim,
			Effect.gen(function* () {
				const admission = yield* Ref.modify(
					state,
					(latest): readonly [RegistrationAdmission, ConnectionState] => {
						if (
							latest._tag !== "Ready" ||
							latest.pending_subscriptions?.[subscription_id] !== claim
						)
							return [{ admitted: false } as const, latest] as const;
						const { [subscription_id]: _removed, ...pending_subscriptions } =
							latest.pending_subscriptions;
						const next = {
							...latest,
							pending_subscriptions,
							subscription_claims: {
								...latest.subscription_claims,
								[subscription_id]: claim,
							},
							subscriptions: {
								...latest.subscriptions,
								[subscription_id]: subscription,
							},
						} satisfies ReadyState;
						return [{ admitted: true, registered: next } as const, next] as const;
					},
				);
				if (!admission.admitted) {
					yield* RefuseRegistration(correlation_id, current);
					return current;
				}
				const { registered } = admission;
				const sent_at = yield* metadata.Now;
				yield* Enqueue({
					correlation_id,
					kind: "subscription.started",
					message_id: yield* metadata.MakeId("message"),
					origin: "backend",
					payload: { stream_id: subscription.stream_id },
					protocol_version: 1,
					schema_version: 1,
					sent_at,
					subscription_id,
				});
				yield* Enqueue(
					MakeSnapshot({
						message_id: yield* metadata.MakeId("message"),
						sent_at,
					}),
				);

				return registered;
			}),
		).pipe(Effect.map(Option.getOrElse(() => current)));
	const StartWithoutSnapshot = (
		correlation_id: string,
		subscription_id: string,
		claim: PendingSubscription,
		current: ReadyState,
		subscription: ProjectionSubscription,
	) =>
		PublishClaim(
			subscription_id,
			claim,
			Effect.gen(function* () {
				const admission = yield* Ref.modify(
					state,
					(latest): readonly [RegistrationAdmission, ConnectionState] => {
						if (
							latest._tag !== "Ready" ||
							latest.pending_subscriptions?.[subscription_id] !== claim
						)
							return [{ admitted: false } as const, latest] as const;
						const { [subscription_id]: _removed, ...pending_subscriptions } =
							latest.pending_subscriptions;
						const next = {
							...latest,
							pending_subscriptions,
							subscription_claims: {
								...latest.subscription_claims,
								[subscription_id]: claim,
							},
							subscriptions: {
								...latest.subscriptions,
								[subscription_id]: subscription,
							},
						} satisfies ReadyState;
						return [{ admitted: true, registered: next } as const, next] as const;
					},
				);
				if (!admission.admitted) {
					yield* RefuseRegistration(correlation_id, current);
					return current;
				}
				const { registered } = admission;
				const started: OutboundControlEnvelope = {
					correlation_id,
					kind: "subscription.started",
					message_id: yield* metadata.MakeId("message"),
					origin: "backend",
					payload: { stream_id: subscription.stream_id },
					protocol_version: 1,
					schema_version: 1,
					sent_at: yield* metadata.Now,
					subscription_id,
				};
				yield* Enqueue(started);
				return registered;
			}),
		).pipe(Effect.map(Option.getOrElse(() => current)));

	return ConnectionProjectionRuntime.of({ Start, StartWithoutSnapshot });
});
