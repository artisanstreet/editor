import { Context, Effect, Fiber, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { SurfaceUsageAggregate } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { RunAuthoritativeSubscription } from "../conversation/subscription";

export type RunUsageState =
	| { readonly _tag: "None" }
	| { readonly _tag: "Loading"; readonly run_id: string }
	| {
			readonly _tag: "Ready";
			readonly aggregate: SurfaceUsageAggregate;
			readonly run_id: string;
	  }
	| { readonly _tag: "Unavailable"; readonly run_id: string };

type ActiveRunUsage = {
	readonly fiber?: Fiber.Fiber<void, unknown>;
	readonly owner_id?: number;
	readonly run_id?: string;
};

export interface RunUsageLease {
	readonly Release: Effect.Effect<void>;
	readonly Select: (run_id: string | undefined) => Effect.Effect<void>;
}

/**
 * Owns the one authoritative usage projection for the run currently visible in
 * the thread route. The client subscription supplies an initial snapshot and
 * reconnect recovery, so components never poll usage beside unrelated thread
 * reads.
 */
export class RunUsageController extends Context.Service<
	RunUsageController,
	{
		readonly Changes: Stream.Stream<RunUsageState>;
		readonly Acquire: (run_id: string | undefined) => Effect.Effect<RunUsageLease>;
	}
>()("Artisan/RunUsageController") {}

export const RunUsageControllerLive = Layer.effect(
	RunUsageController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const active = yield* Ref.make<ActiveRunUsage>({});
		const next_owner_id = yield* Ref.make(0);
		const state = yield* SubscriptionRef.make<RunUsageState>({ _tag: "None" });
		const select_lock = yield* Semaphore.make(1);

		const Consume = (owner_id: number, run_id: string) =>
			Effect.gen(function* () {
				yield* RunAuthoritativeSubscription(
					client.SubscribeSurfaceUsageAggregate({ scope: "run", scope_id: run_id }),
					(update) =>
						Effect.gen(function* () {
							const selected = yield* Ref.get(active);
							if (selected.owner_id !== owner_id || selected.run_id !== run_id)
								return;
							yield* SubscriptionRef.set(state, {
								aggregate: update.snapshot.aggregate,
								run_id,
								_tag: "Ready",
							});
						}),
					Effect.gen(function* () {
						const selected = yield* Ref.get(active);
						if (selected.owner_id !== owner_id || selected.run_id !== run_id) return;
						yield* SubscriptionRef.set(state, { run_id, _tag: "Unavailable" });
					}),
				);
			});

		const Select = (owner_id: number, run_id: string | undefined) =>
			Effect.gen(function* () {
				yield* select_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* Ref.get(active);
						if (current.owner_id === owner_id && current.run_id === run_id) return;
						if (current.fiber !== undefined) yield* Fiber.interrupt(current.fiber);

						if (run_id === undefined) {
							yield* Ref.set(active, { owner_id });
							yield* SubscriptionRef.set(state, { _tag: "None" });
							return;
						}

						yield* SubscriptionRef.set(state, { run_id, _tag: "Loading" });
						yield* Ref.set(active, { owner_id, run_id });
						const fiber = yield* Effect.forkIn(
							Consume(owner_id, run_id),
							controller_scope,
						);
						yield* Ref.set(active, { fiber, owner_id, run_id });
					}),
				);
			});

		const Release = (owner_id: number) =>
			Effect.gen(function* () {
				yield* select_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* Ref.get(active);
						if (current.owner_id !== owner_id) return;
						if (current.fiber !== undefined) yield* Fiber.interrupt(current.fiber);
						yield* Ref.set(active, {});
						yield* SubscriptionRef.set(state, { _tag: "None" });
					}),
				);
			});

		const Acquire = (run_id: string | undefined) =>
			Effect.gen(function* () {
				const owner_id = yield* Ref.updateAndGet(next_owner_id, (current) => current + 1);
				yield* Select(owner_id, run_id);
				return {
					Release: Release(owner_id),
					Select: (next_run_id) =>
						Effect.gen(function* () {
							yield* Select(owner_id, next_run_id);
						}),
				} satisfies RunUsageLease;
			});

		return RunUsageController.of({ Acquire, Changes: SubscriptionRef.changes(state) });
	}),
);
