import { Effect, Stream, SubscriptionRef } from "effect";

interface EngineUsageRefreshState {
	readonly claims: ReadonlyMap<string, number>;
	readonly next_claim_id: number;
}

interface EngineUsageRefreshClaim {
	readonly claim_id: number;
	readonly engine_id: string;
}

/** Coordinates provider refresh ownership across overlapping component fibers. */
export interface EngineUsageRefreshController {
	readonly Changes: Stream.Stream<ReadonlySet<string>>;
	readonly Current: Effect.Effect<ReadonlySet<string>>;
	readonly Refresh: <A, E, R, B, E2, R2>(
		requested_engine_ids: ReadonlyArray<string>,
		refresh: (engine_id: string) => Effect.Effect<A, E, R>,
		on_idle: Effect.Effect<B, E2, R2>,
	) => Effect.Effect<boolean, E | E2, R | R2>;
}

/**
 * Creates one component-scoped refresh coordinator. Claims are serialized by
 * `SubscriptionRef`, duplicate provider requests are skipped, and every claim
 * carries a generation so cleanup cannot release a newer refresh for the same
 * provider. Every claim is released when its refresh succeeds, fails, or is
 * interrupted.
 */
export const MakeEngineUsageRefreshController = Effect.gen(function* () {
	const refreshing = yield* SubscriptionRef.make<EngineUsageRefreshState>({
		claims: new Map(),
		next_claim_id: 0,
	});
	const EngineIds = (state: EngineUsageRefreshState): ReadonlySet<string> =>
		new Set(state.claims.keys());

	const Current = SubscriptionRef.get(refreshing).pipe(Effect.map(EngineIds));
	const Claim = (requested_engine_ids: ReadonlyArray<string>) =>
		SubscriptionRef.modify(refreshing, (current) => {
			const claims = new Map(current.claims);
			const claimed: Array<EngineUsageRefreshClaim> = [];
			let next_claim_id = current.next_claim_id;
			for (const engine_id of new Set(requested_engine_ids)) {
				if (claims.has(engine_id)) continue;
				claimed.push({ claim_id: next_claim_id, engine_id });
				claims.set(engine_id, next_claim_id);
				next_claim_id += 1;
			}
			if (claimed.length === 0) return [claimed, current] as const;

			return [claimed, { claims, next_claim_id }] as const;
		});
	const Release = (claim: EngineUsageRefreshClaim) =>
		SubscriptionRef.modify(refreshing, (current) => {
			if (current.claims.get(claim.engine_id) !== claim.claim_id) {
				return [undefined, current] as const;
			}

			const claims = new Map(current.claims);
			claims.delete(claim.engine_id);
			return [undefined, { ...current, claims }] as const;
		});
	const ReleaseAll = (claims: ReadonlyArray<EngineUsageRefreshClaim>) =>
		Effect.forEach(claims, Release, { discard: true });
	const Refresh = <A, E, R, B, E2, R2>(
		requested_engine_ids: ReadonlyArray<string>,
		refresh: (engine_id: string) => Effect.Effect<A, E, R>,
		on_idle: Effect.Effect<B, E2, R2>,
	) =>
		Effect.gen(function* () {
			const claimed = yield* Claim(requested_engine_ids);
			yield* Effect.forEach(
				claimed,
				(claim) => refresh(claim.engine_id).pipe(Effect.ensuring(Release(claim))),
				{ concurrency: "unbounded", discard: true },
			).pipe(
				/** Concurrent workers can outlive parent interruption; release the whole claim set too. */
				Effect.ensuring(ReleaseAll(claimed)),
			);

			const idle = (yield* Current).size === 0;
			if (idle) yield* on_idle;

			return idle;
		});

	return {
		Changes: SubscriptionRef.changes(refreshing).pipe(Stream.map(EngineIds)),
		Current,
		Refresh,
	} satisfies EngineUsageRefreshController;
});
