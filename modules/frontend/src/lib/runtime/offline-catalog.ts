import { model_manifest } from "@artisan/catalog";
import type { RuntimeCatalog } from "@artisan/protocol";
import { Effect } from "effect";

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
	Effect.gen(function* () {
		return yield* catalog;
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return OfflineRuntimeCatalog;
			}),
		),
	);
