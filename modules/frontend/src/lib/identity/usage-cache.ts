import { Context, Duration, Effect, Layer, Option, Result } from "effect";
import * as KeyValueStore from "effect/unstable/persistence/KeyValueStore";
import { EngineUsageSnapshot } from "@artisan/protocol";

/**
 * Last-known provider-account usage, persisted so the identity dropdown can
 * render instantly on open — refreshing in the background — instead of
 * showing skeletons while a fetch (which may spawn a provider CLI) is in
 * flight. Host identity is not cached here: it is fast and process-cached
 * server-side.
 */
export const EngineUsageCacheStorageKey = "artisan.engine-usage-cache";

/** Mirrors Forge's provider-usage freshness boundary without duplicating a magic duration. */
const engine_usage_refresh_window_ms = Duration.toMillis(Duration.minutes(3));

/** A missing, invalid, or three-minute-old snapshot should be refreshed when the menu opens. */
export const engine_usage_refresh_is_due = (
	snapshot: EngineUsageSnapshot | undefined,
	now_ms: number,
): boolean => {
	if (snapshot === undefined) return true;

	const fetched_at_ms = Date.parse(snapshot.fetched_at);

	return (
		!Number.isFinite(fetched_at_ms) || now_ms - fetched_at_ms >= engine_usage_refresh_window_ms
	);
};

export class EngineUsageCache extends Context.Service<
	EngineUsageCache,
	{
		readonly Load: Effect.Effect<Option.Option<EngineUsageSnapshot>>;
		readonly Save: (snapshot: EngineUsageSnapshot) => Effect.Effect<void>;
	}
>()("Artisan/EngineUsageCache") {}

/** Requires an ambient `KeyValueStore`; tests provide `KeyValueStore.layerMemory`. */
export const EngineUsageCacheLive = Layer.effect(
	EngineUsageCache,
	Effect.gen(function* () {
		const store = yield* KeyValueStore.KeyValueStore;
		const schema_store = KeyValueStore.toSchemaStore(store, EngineUsageSnapshot);

		const RemoveCorrupt = Effect.gen(function* () {
			yield* store.remove(EngineUsageCacheStorageKey).pipe(Effect.result);
		});

		const Load = Effect.gen(function* () {
			const stored_result = yield* schema_store
				.get(EngineUsageCacheStorageKey)
				.pipe(Effect.result);

			if (Result.isFailure(stored_result)) {
				yield* RemoveCorrupt;

				return Option.none();
			}

			return stored_result.success;
		});

		const Save = (snapshot: EngineUsageSnapshot) =>
			Effect.gen(function* () {
				yield* schema_store.set(EngineUsageCacheStorageKey, snapshot).pipe(Effect.result);
			});

		return EngineUsageCache.of({ Load, Save });
	}),
);

/** Falls back to an in-memory store when browser storage acquisition defects (private mode, disabled storage). */
const RecoverKeyValueStore = Layer.catchCause(() => KeyValueStore.layerMemory);

/** Self-contained layer backed by real browser storage; provide this at the component call site. */
export const EngineUsageCacheBrowserLive = EngineUsageCacheLive.pipe(
	Layer.provide(KeyValueStore.layerStorage(() => localStorage).pipe(RecoverKeyValueStore)),
);
