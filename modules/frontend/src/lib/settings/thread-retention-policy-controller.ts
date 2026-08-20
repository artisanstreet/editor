import { Context, Deferred, Effect, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import type { ThreadRetentionPolicy } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

export type ThreadRetentionPolicyState =
	| { readonly _tag: "Loading" }
	| { readonly _tag: "Ready"; readonly policy: ThreadRetentionPolicy }
	| { readonly _tag: "Unverified" };

type Flight = {
	readonly deferred: Deferred.Deferred<ThreadRetentionPolicy, ArtisanClientError>;
	readonly generation: number;
};

export class ThreadRetentionPolicyController extends Context.Service<
	ThreadRetentionPolicyController,
	{
		readonly Changes: Stream.Stream<ThreadRetentionPolicyState>;
		readonly Current: Effect.Effect<ThreadRetentionPolicyState>;
		readonly Refresh: Effect.Effect<ThreadRetentionPolicy, ArtisanClientError>;
		readonly Save: (
			policy: ThreadRetentionPolicy,
		) => Effect.Effect<ThreadRetentionPolicy, ArtisanClientError>;
	}
>()("Artisan/ThreadRetentionPolicyController") {}

export const ThreadRetentionPolicyControllerLive = Layer.effect(
	ThreadRetentionPolicyController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<ThreadRetentionPolicyState>({ _tag: "Loading" });
		const inflight = yield* Ref.make<Flight | undefined>(undefined);
		const generation = yield* Ref.make(0);
		const save_lock = yield* Semaphore.make(1);
		const Complete = (flight: Flight) =>
			client.GetThreadRetentionPolicy.pipe(
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Ref.update(inflight, (current) =>
							current === flight ? undefined : current,
						);
						const latest = yield* Ref.get(generation);
						if (latest === flight.generation) {
							yield* exit._tag === "Success"
								? SubscriptionRef.set(state, {
										_tag: "Ready",
										policy: exit.value,
									})
								: SubscriptionRef.set(state, { _tag: "Unverified" });
						}
						yield* Deferred.done(flight.deferred, exit);
					}),
				),
				Effect.asVoid,
			);
		const Refresh = Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				if (current._tag === "Ready") return current.policy;
				const request_generation = yield* Ref.get(generation);
				const candidate = {
					deferred: yield* Deferred.make<ThreadRetentionPolicy, ArtisanClientError>(),
					generation: request_generation,
				} satisfies Flight;
				const claim = yield* Ref.modify(inflight, (current) => {
					if (current !== undefined && current.generation === request_generation) {
						return [current, current] as const;
					}
					return [candidate, candidate] as const;
				});
				if (claim === candidate) yield* Effect.forkIn(Complete(candidate), scope);
				return yield* restore(Deferred.await(claim.deferred));
			}),
		);
		const Save = (policy: ThreadRetentionPolicy) =>
			save_lock.withPermit(
				Effect.gen(function* () {
					const write_generation = yield* Ref.updateAndGet(
						generation,
						(current) => current + 1,
					);
					yield* client.UpdateThreadRetentionPolicy(policy).pipe(
						Effect.catch((error) =>
							Effect.gen(function* () {
								yield* Ref.get(generation).pipe(
									Effect.flatMap((latest) =>
										latest === write_generation
											? SubscriptionRef.set(state, { _tag: "Unverified" })
											: Effect.void,
									),
								);
								yield* Effect.forkIn(
									Refresh.pipe(Effect.catch(() => Effect.void)),
									scope,
								);
								return yield* Effect.fail(error);
							}),
						),
					);
					yield* Ref.get(generation).pipe(
						Effect.flatMap((latest) =>
							latest === write_generation
								? SubscriptionRef.set(state, { _tag: "Ready", policy })
								: Effect.void,
						),
					);
					return policy;
				}),
			);
		return ThreadRetentionPolicyController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Refresh,
			Save,
		});
	}),
);
