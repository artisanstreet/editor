import { Context, Effect, Fiber, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { HostIdentitySnapshot } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

type ActiveRefresh = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly generation: number;
};

/**
 * Retains the one host identity shared by shell and thread context surfaces.
 * Refresh work belongs to this app-scoped layer, so a route or menu closing
 * cannot interrupt the request after it has been admitted.
 */
export class HostIdentityController extends Context.Service<
	HostIdentityController,
	{
		readonly Changes: Stream.Stream<HostIdentitySnapshot | undefined>;
		readonly Current: Effect.Effect<HostIdentitySnapshot | undefined>;
		readonly Refresh: Effect.Effect<void>;
	}
>()("Artisan/HostIdentityController") {}

export const HostIdentityControllerLive = Layer.effect(
	HostIdentityController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const active = yield* Ref.make<ActiveRefresh | undefined>(undefined);
		const generation = yield* Ref.make(0);
		const refresh_lock = yield* Semaphore.make(1);
		const state = yield* SubscriptionRef.make<HostIdentitySnapshot | undefined>(undefined);

		const Complete = (request_generation: number) =>
			Ref.update(active, (current) =>
				current?.generation === request_generation ? undefined : current,
			);

		const Load = (request_generation: number) =>
			client.GetHostIdentity.pipe(
				Effect.flatMap((identity) =>
					Effect.gen(function* () {
						const current_generation = yield* Ref.get(generation);
						if (current_generation !== request_generation) return;
						yield* SubscriptionRef.set(state, identity);
					}),
				),
				Effect.catch(() => Effect.void),
				Effect.ensuring(Complete(request_generation)),
			);

		const Refresh = Effect.uninterruptible(
			refresh_lock.withPermit(
				Effect.gen(function* () {
					if ((yield* SubscriptionRef.get(state)) !== undefined) return;
					if ((yield* Ref.get(active)) !== undefined) return;
					const request_generation = yield* Ref.updateAndGet(
						generation,
						(current) => current + 1,
					);
					yield* Ref.set(active, { generation: request_generation });
					const fiber = yield* Effect.forkIn(Load(request_generation), controller_scope);
					yield* Ref.update(active, (current) =>
						current?.generation === request_generation
							? { fiber, generation: request_generation }
							: current,
					);
				}),
			),
		);

		return HostIdentityController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Refresh,
		});
	}),
);
