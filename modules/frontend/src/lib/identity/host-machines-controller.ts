import { Context, Effect, Fiber, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { HostMachineConnectOutcome, HostMachinesSnapshot } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";

type ActiveRefresh = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly generation: number;
};

/**
 * Retains the one execution-machine list shared by thread context surfaces.
 * Refresh work belongs to this app-scoped layer, so a route or menu closing
 * cannot interrupt the request after it has been admitted.
 */
export class HostMachinesController extends Context.Service<
	HostMachinesController,
	{
		readonly Changes: Stream.Stream<HostMachinesSnapshot | undefined>;
		/** Terminal outcome, or undefined when the request itself could not complete. */
		readonly Connect: (
			machine_id: string,
		) => Effect.Effect<HostMachineConnectOutcome | undefined>;
		readonly Current: Effect.Effect<HostMachinesSnapshot | undefined>;
		readonly Refresh: Effect.Effect<void>;
	}
>()("Artisan/HostMachinesController") {}

export const HostMachinesControllerLive = Layer.effect(
	HostMachinesController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const active = yield* Ref.make<ActiveRefresh | undefined>(undefined);
		const generation = yield* Ref.make(0);
		const refresh_lock = yield* Semaphore.make(1);
		const state = yield* SubscriptionRef.make<HostMachinesSnapshot | undefined>(undefined);

		const Complete = (request_generation: number) =>
			Ref.update(active, (current) =>
				current?.generation === request_generation ? undefined : current,
			);

		const Load = (request_generation: number) =>
			client.GetHostMachines.pipe(
				Effect.flatMap((machines) =>
					Effect.gen(function* () {
						const current_generation = yield* Ref.get(generation);
						if (current_generation !== request_generation) return;
						yield* SubscriptionRef.set(state, machines);
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

		const Connect = (machine_id: string) =>
			client
				.ConnectHostMachine({ machine_id })
				.pipe(
					Effect.catch(() =>
						Effect.succeed(undefined as HostMachineConnectOutcome | undefined),
					),
				);

		return HostMachinesController.of({
			Changes: SubscriptionRef.changes(state),
			Connect,
			Current: SubscriptionRef.get(state),
			Refresh,
		});
	}),
);
