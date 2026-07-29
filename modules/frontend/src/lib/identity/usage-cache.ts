import { Context, Effect, Layer, Option, Result } from "effect";
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
