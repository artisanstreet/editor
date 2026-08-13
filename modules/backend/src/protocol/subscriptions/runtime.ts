import { Context, Effect, Ref } from "effect";

import type { OutboundControlEnvelope } from "@artisan/protocol";

import { RuntimeMetadata } from "../../runtime/metadata";
import type { ProjectionSubscription, ReadyState } from "../connection-state";
import { ConnectionSubscriptionControl } from "./control";

interface ProjectionMetadata {
	readonly message_id: string;
	readonly sent_at: string;
}

export class ConnectionProjectionRuntime extends Context.Service<
	ConnectionProjectionRuntime,
	{
		readonly Start: (
			correlation_id: string,
			subscription_id: string,
			current: ReadyState,
			subscription: ProjectionSubscription,
			MakeSnapshot: (metadata: ProjectionMetadata) => OutboundControlEnvelope,
		) => Effect.Effect<ReadyState>;
		readonly StartWithoutSnapshot: (
			correlation_id: string,
			subscription_id: string,
			current: ReadyState,
			subscription: ProjectionSubscription,
		) => Effect.Effect<ReadyState>;
	}
>()("Artisan/ConnectionProjectionRuntime") {}

export const MakeConnectionProjectionRuntime = Effect.gen(function* () {
	const metadata = yield* RuntimeMetadata;
	const { state, Enqueue } = yield* ConnectionSubscriptionControl;

	const Start = (
		correlation_id: string,
		subscription_id: string,
		current: ReadyState,
		subscription: ProjectionSubscription,
		MakeSnapshot: (metadata: ProjectionMetadata) => OutboundControlEnvelope,
	) =>
		Effect.gen(function* () {
			const registered = {
				...current,
				subscriptions: {
					...current.subscriptions,
					[subscription_id]: subscription,
				},
			} satisfies ReadyState;
			const sent_at = yield* metadata.Now;
			yield* Ref.set(state, registered);
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
		});
	const StartWithoutSnapshot = (
		correlation_id: string,
		subscription_id: string,
		current: ReadyState,
		subscription: ProjectionSubscription,
	) =>
		Effect.gen(function* () {
			const registered = {
				...current,
				subscriptions: { ...current.subscriptions, [subscription_id]: subscription },
			} satisfies ReadyState;
			yield* Ref.set(state, registered);
			yield* Enqueue({
				correlation_id,
				kind: "subscription.started",
				message_id: yield* metadata.MakeId("message"),
				origin: "backend",
				payload: { stream_id: subscription.stream_id },
				protocol_version: 1,
				schema_version: 1,
				sent_at: yield* metadata.Now,
				subscription_id,
			});
			return registered;
		});

	return ConnectionProjectionRuntime.of({ Start, StartWithoutSnapshot });
});
