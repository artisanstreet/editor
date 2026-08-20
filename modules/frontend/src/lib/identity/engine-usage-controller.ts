import { Clock, Context, Deferred, Effect, Layer, Ref, Stream, SubscriptionRef } from "effect";

import type { EngineUsageReport, EngineUsageSnapshot } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";

const fresh_for_ms = 3 * 60_000;

export interface EngineUsageEntry {
	/** When this engine answered — its own reading, never the newest engine's. */
	readonly fetched_at_ms: number;
	/**
	 * Why this engine has no report, when the query itself failed.
	 *
	 * A provider that fails behind a reachable Forge still answers with a
	 * failure-shaped report, so this is the transport case only. Recorded rather
	 * than dropped because an engine with no entry is indistinguishable from one
	 * that was never asked, and the menu drew that as "not authenticated".
	 */
	readonly failure?: string;
	readonly report: EngineUsageReport | undefined;
}

export interface EngineUsageState {
	readonly entries: ReadonlyMap<string, EngineUsageEntry>;
	readonly refreshing_engine_ids: ReadonlySet<string>;
}

type Flight = {
	readonly deferred: Deferred.Deferred<EngineUsageEntry, ArtisanClientError>;
	readonly force: boolean;
};

const InitialState: EngineUsageState = {
	entries: new Map(),
	refreshing_engine_ids: new Set(),
};

const IsFresh = (entry: EngineUsageEntry, now_ms: number) =>
	now_ms - entry.fetched_at_ms < fresh_for_ms;

/**
 * Retains per-engine usage at application scope. Effect's generic Cache is
 * ideal for ordinary keyed sharing, but this controller also has to publish
 * report state and let a manual forced refresh supersede a non-forced flight.
 */
export class EngineUsageController extends Context.Service<
	EngineUsageController,
	{
		readonly Changes: Stream.Stream<EngineUsageState>;
		readonly Current: Effect.Effect<EngineUsageState>;
		readonly Load: (
			engine_id: string,
			options?: { readonly force?: boolean },
		) => Effect.Effect<EngineUsageEntry, ArtisanClientError>;
		/** Hydrates the controller from the sidebar's persisted per-engine cache. */
		readonly Seed: (snapshot: EngineUsageSnapshot) => Effect.Effect<void>;
	}
>()("Artisan/EngineUsageController") {}

export const EngineUsageControllerLive = Layer.effect(
	EngineUsageController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const controller_scope = yield* Effect.scope;
		const state = yield* SubscriptionRef.make<EngineUsageState>(InitialState);
		const flights = yield* Ref.make<ReadonlyMap<string, Flight>>(new Map());

		const Publish = (engine_id: string, entry: EngineUsageEntry) =>
			SubscriptionRef.modify(state, (current) => {
				const retained = current.entries.get(engine_id);
				if (retained !== undefined && retained.fetched_at_ms > entry.fetched_at_ms) {
					return [retained, current] as const;
				}
				return [
					entry,
					{ ...current, entries: new Map(current.entries).set(engine_id, entry) },
				] as const;
			});
		const Complete = (engine_id: string, flight: Flight) =>
			Effect.gen(function* () {
				const was_current = yield* Ref.modify(flights, (current) => {
					if (current.get(engine_id) !== flight) return [false, current] as const;
					const next = new Map(current);
					next.delete(engine_id);
					return [true, next] as const;
				});
				if (!was_current) return;
				yield* SubscriptionRef.update(state, (current) => {
					const refreshing_engine_ids = new Set(current.refreshing_engine_ids);
					refreshing_engine_ids.delete(engine_id);
					return { ...current, refreshing_engine_ids };
				});
			});
		const CompleteFlight = (engine_id: string, flight: Flight) =>
			client.GetEngineUsage({ engine_id, ...(flight.force ? { force: true } : {}) }).pipe(
				Effect.map((snapshot) => ({
					fetched_at_ms: Date.parse(snapshot.fetched_at),
					report: snapshot.engines.find((candidate) => candidate.engine_id === engine_id),
				})),
				/**
				 * A failed query still publishes, so this engine reports its own
				 * condition rather than leaving a hole the menu reads as an engine
				 * nobody asked about. The awaiting caller still receives the failure.
				 */
				Effect.tapError((cause) =>
					Clock.currentTimeMillis.pipe(
						Effect.flatMap((now_ms) =>
							Publish(engine_id, {
								failure: cause.message,
								fetched_at_ms: now_ms,
								report: undefined,
							}),
						),
					),
				),
				Effect.flatMap((entry) => Publish(engine_id, entry)),
				Effect.exit,
				Effect.flatMap((exit) =>
					Effect.gen(function* () {
						yield* Complete(engine_id, flight);
						yield* Deferred.done(flight.deferred, exit);
					}),
				),
			);
		const Seed = (snapshot: EngineUsageSnapshot) =>
			Effect.gen(function* () {
				const fetched_at_ms = Date.parse(snapshot.fetched_at);
				if (!Number.isFinite(fetched_at_ms)) return;
				yield* SubscriptionRef.update(state, (current) => {
					const entries = new Map(current.entries);
					for (const report of snapshot.engines) {
						const retained = entries.get(report.engine_id);
						if (retained === undefined || retained.fetched_at_ms < fetched_at_ms) {
							entries.set(report.engine_id, { fetched_at_ms, report });
						}
					}
					return { ...current, entries };
				});
			});

		const Load: (
			engine_id: string,
			options?: { readonly force?: boolean },
		) => Effect.Effect<EngineUsageEntry, ArtisanClientError> = (engine_id, options = {}) =>
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					const force = options.force === true;
					const now_ms = yield* Clock.currentTimeMillis;
					const current = yield* SubscriptionRef.get(state);
					const retained = current.entries.get(engine_id);
					if (!force && retained !== undefined && IsFresh(retained, now_ms))
						return retained;

					const candidate = yield* Deferred.make<EngineUsageEntry, ArtisanClientError>();
					const claim = yield* Ref.modify(flights, (current_flights) => {
						const current_flight = current_flights.get(engine_id);
						if (current_flight !== undefined)
							return [current_flight, current_flights] as const;
						const flight = { deferred: candidate, force } satisfies Flight;
						return [flight, new Map(current_flights).set(engine_id, flight)] as const;
					});
					if (claim.deferred !== candidate) {
						/** A manual refresh never accepts a preceding non-forced backend-cache read. */
						if (force && !claim.force) {
							yield* restore(Deferred.await(claim.deferred)).pipe(Effect.ignore);
							return yield* Load(engine_id, { force: true });
						}
						return yield* restore(Deferred.await(claim.deferred));
					}
					yield* SubscriptionRef.update(state, (current_state) => ({
						...current_state,
						refreshing_engine_ids: new Set(current_state.refreshing_engine_ids).add(
							engine_id,
						),
					}));
					yield* Effect.forkIn(CompleteFlight(engine_id, claim), controller_scope);
					return yield* restore(Deferred.await(claim.deferred));
				}),
			);

		return EngineUsageController.of({
			Changes: SubscriptionRef.changes(state),
			Current: SubscriptionRef.get(state),
			Load,
			Seed,
		});
	}),
);
