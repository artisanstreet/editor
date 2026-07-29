import { describe, expect, layer } from "@effect/vitest";
import { Effect, Option } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";
import type { EngineUsageSnapshot } from "@artisan/protocol";

import {
	EngineUsageCache,
	EngineUsageCacheLive,
	EngineUsageCacheStorageKey,
} from "../../modules/frontend/src/lib/identity/usage-cache";

const sample_snapshot: EngineUsageSnapshot = {
	engines: [
		{
			authentication: "authenticated",
			display_name: "Claude",
			engine_id: "claude",
			windows: [
				{
					id: "five_hour",
					kind: "session",
					percent_used: 42,
				},
			],
		},
	],
	fetched_at: "2026-07-29T00:00:00.000Z",
};

describe("engine usage cache", () => {
	layer(KeyValueStore.layerMemory)((it) => {
		it.layer(EngineUsageCacheLive)((it) => {
			it.effect("resolves to no cache when nothing has been saved", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;

					expect(yield* cache.Load).toEqual(Option.none());
				}),
			);

			it.effect("round-trips a saved usage snapshot", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;
					yield* cache.Save(sample_snapshot);

					expect(yield* cache.Load).toEqual(Option.some(sample_snapshot));
				}),
			);

			it.effect("resolves to no cache and repairs corrupt stored JSON", () =>
				Effect.gen(function* () {
					const store = yield* KeyValueStore.KeyValueStore;
					const cache = yield* EngineUsageCache;

					yield* store.clear;
					yield* store.set(EngineUsageCacheStorageKey, "{broken");

					expect(yield* cache.Load).toEqual(Option.none());
					expect(yield* store.get(EngineUsageCacheStorageKey)).toBeUndefined();
				}),
			);

			it.effect(
				"resolves to no cache and repairs a stored value failing schema validation",
				() =>
					Effect.gen(function* () {
						const store = yield* KeyValueStore.KeyValueStore;
						const cache = yield* EngineUsageCache;

						yield* store.clear;
						yield* store.set(
							EngineUsageCacheStorageKey,
							JSON.stringify({
								engines: "not-an-array",
								fetched_at: "2026-07-29T00:00:00.000Z",
							}),
						);

						expect(yield* cache.Load).toEqual(Option.none());
						expect(yield* store.get(EngineUsageCacheStorageKey)).toBeUndefined();
					}),
			);
		});
	});
});
