import { Context, Deferred, Effect, Layer, Ref } from "effect";

import type { ThreadOpenSnapshot } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";
import { ThreadRouteId } from "../root/thread-navigation";

/**
 * How many thread aggregates stay resident.
 *
 * Two retained only the immediate back step, so moving between the handful of
 * threads actually being worked on re-fetched every single time and paid a
 * skeleton for a thread visited a minute ago. Six covers a working set without
 * turning the cache into a second copy of the transcript store: entries are
 * evicted least-recently-used, and each one is an aggregate the renderer has
 * already materialised once.
 */
const maximum_retained_threads = 6;

type ThreadOpenFlight = Deferred.Deferred<ThreadOpenSnapshot, ArtisanClientError>;

interface ThreadOpenControl {
	readonly entries: ReadonlyMap<string, ThreadOpenSnapshot>;
	readonly in_flight: ReadonlyMap<string, ThreadOpenFlight>;
}

type ThreadOpenClaim =
	| { readonly _tag: "Cached"; readonly snapshot: ThreadOpenSnapshot }
	| { readonly _tag: "Follower"; readonly deferred: ThreadOpenFlight }
	| { readonly _tag: "Leader"; readonly deferred: ThreadOpenFlight };

/**
 * Retains the two most recently visited thread aggregates and owns cold-open
 * single-flight work. Back/forward navigation can therefore paint immediately,
 * while a genuinely cold thread paints a route skeleton instead of retaining
 * the previous route until Forge answers.
 */
export class ThreadOpenController extends Context.Service<
	ThreadOpenController,
	{
		readonly Current: (thread_id: string) => Effect.Effect<ThreadOpenSnapshot | undefined>;
		readonly Open: (thread_id: string) => Effect.Effect<ThreadOpenSnapshot, ArtisanClientError>;
		/**
		 * Warms a thread the reader has shown intent toward but has not opened.
		 * Returns as soon as the work is claimed rather than when it lands, so a
		 * pointer resting on a row never blocks on the network, and the open that
		 * follows finds the aggregate already there. Failure is silent: nothing
		 * was asked for yet, and the real open reports its own errors.
		 */
		readonly Prefetch: (thread_id: string) => Effect.Effect<void>;
		readonly Publish: (snapshot: ThreadOpenSnapshot) => Effect.Effect<ThreadOpenSnapshot>;
	}
>()("Artisan/ThreadOpenController") {}

export const ThreadOpenControllerLive = Layer.effect(
	ThreadOpenController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const control = yield* Ref.make<ThreadOpenControl>({
			entries: new Map(),
			in_flight: new Map(),
		});

		const Retain = (key: string, snapshot: ThreadOpenSnapshot) =>
			Ref.updateAndGet(control, (current) => {
				const entries = new Map(current.entries);
				entries.delete(key);
				entries.set(key, snapshot);
				while (entries.size > maximum_retained_threads) {
					const oldest = entries.keys().next().value;
					if (oldest === undefined) break;
					entries.delete(oldest);
				}
				return { ...current, entries };
			}).pipe(Effect.as(snapshot));

		const Publish = (snapshot: ThreadOpenSnapshot) =>
			Retain(ThreadRouteId(snapshot.thread.thread_id), snapshot);

		const Current = (thread_id: string) =>
			Effect.gen(function* () {
				const key = ThreadRouteId(thread_id);
				return yield* Ref.modify(control, (current) => {
					const snapshot = current.entries.get(key);
					if (snapshot === undefined) return [undefined, current] as const;
					const entries = new Map(current.entries);
					entries.delete(key);
					entries.set(key, snapshot);
					return [snapshot, { ...current, entries }] as const;
				});
			});

		const Complete = (key: string, deferred: ThreadOpenFlight) =>
			client.GetThreadOpen(key).pipe(
				Effect.flatMap(Publish),
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Ref.update(control, (current) => {
							if (current.in_flight.get(key) !== deferred) return current;
							const in_flight = new Map(current.in_flight);
							in_flight.delete(key);
							return { ...current, in_flight };
						});
						yield* Deferred.done(deferred, exit);
					}),
				),
				Effect.asVoid,
			);

		const Open = (thread_id: string) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const key = ThreadRouteId(thread_id);
					const candidate = yield* Deferred.make<
						ThreadOpenSnapshot,
						ArtisanClientError
					>();
					const claim = yield* Ref.modify(
						control,
						(current): readonly [ThreadOpenClaim, ThreadOpenControl] => {
							const cached = current.entries.get(key);
							if (cached !== undefined) {
								const entries = new Map(current.entries);
								entries.delete(key);
								entries.set(key, cached);
								return [
									{ _tag: "Cached", snapshot: cached },
									{ ...current, entries },
								];
							}
							const active = current.in_flight.get(key);
							if (active !== undefined)
								return [{ _tag: "Follower", deferred: active }, current];
							const in_flight = new Map(current.in_flight).set(key, candidate);
							return [
								{ _tag: "Leader", deferred: candidate },
								{ ...current, in_flight },
							];
						},
					);

					if (claim._tag === "Cached") return claim.snapshot;
					if (claim._tag === "Leader") {
						yield* Effect.forkIn(Complete(key, claim.deferred), controller_scope);
					}
					return yield* restore(Deferred.await(claim.deferred));
				}),
			);

		/**
		 * Single-flight makes this safe to call as often as the pointer moves: a
		 * warm thread resolves from the cache without touching the network, and a
		 * cold one already in flight joins that request rather than starting
		 * another.
		 */
		const Prefetch = (thread_id: string) =>
			Open(thread_id).pipe(Effect.ignore, Effect.forkIn(controller_scope), Effect.asVoid);

		return ThreadOpenController.of({ Current, Open, Prefetch, Publish });
	}),
);
