import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { Effect, Stream } from "effect";

/**
 * Forge owns the runtime catalog, but the manifest inside it is static
 * application data compiled into the client, so a disconnected renderer can
 * still present every engine, model, and permission option truthfully.
 *
 * What a disconnected renderer cannot know is which harnesses this machine has
 * actually registered, and it has no session to run one through. The offline
 * catalog therefore carries the full manifest with nothing runnable, which is
 * the honest answer and the same condition the composer already refuses to
 * send under.
 */
export const OfflineRuntimeCatalog: RuntimeCatalog = {
	manifest: model_manifest,
	runnable_harness_ids: [],
};

/** Whether a surface is presenting the manifest without a Forge behind it. */
export const IsOfflineRuntimeCatalog = (catalog: RuntimeCatalog): boolean =>
	catalog === OfflineRuntimeCatalog;

/**
 * Substitutes the offline catalog when Forge cannot answer, so the model
 * pickers and composer stay readable while disconnected instead of failing
 * their component programs.
 */
export const WithOfflineRuntimeCatalog = <E, R>(
	catalog: Effect.Effect<RuntimeCatalog, E, R>,
): Effect.Effect<RuntimeCatalog, never, R> =>
	catalog.pipe(Effect.catch(() => Effect.succeed(OfflineRuntimeCatalog)));

/**
 * Emits the catalog available at mount, switches to the offline sentinel while
 * Forge is disconnected, and replaces it on every successful connection. A
 * failed ready-state refresh emits nothing, preserving the preceding state.
 *
 * The client remains an Effect service dependency rather than being threaded
 * through component helpers as an ordinary parameter.
 */
export const RuntimeCatalogChanges: Stream.Stream<RuntimeCatalog, never, ArtisanClient> =
	Stream.unwrap(
		Effect.gen(function* () {
			const client = yield* ArtisanClient;
			const initial = yield* WithOfflineRuntimeCatalog(client.GetRuntimeCatalog);
			const ready_states = Stream.concat(
				Stream.fromEffect(client.ConnectionState),
				client.ConnectionChanges,
			);
			const catalog_states = ready_states.pipe(
				Stream.mapEffect((state) =>
					state.phase === "ready"
						? client.GetRuntimeCatalog.pipe(Effect.result)
						: Effect.succeed(OfflineRuntimeCatalog).pipe(Effect.result),
				),
				Stream.filterMap((catalog) => catalog),
			);

			return Stream.concat(Stream.succeed(initial), catalog_states).pipe(
				Stream.changesWith((left, right) => left === right),
			);
		}),
	);
