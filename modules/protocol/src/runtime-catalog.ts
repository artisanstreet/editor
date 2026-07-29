import { ModelManifest } from "@artisan/catalog";
import { Schema } from "effect";

/** Describes the model and harness capabilities exposed by this Forge process. */
export const RuntimeCatalog = Schema.Struct({
	default_model_id: Schema.optionalKey(Schema.NonEmptyString),
	manifest: ModelManifest,
	/**
	 * Engines registered in this Forge process, by descriptor id. The manifest
	 * always carries the full catalog so pickers can preview every model; a
	 * harness outside this list cannot start a session. Ids stay unconstrained
	 * because registries may host engines outside the curated catalog.
	 */
	runnable_harness_ids: Schema.Array(Schema.NonEmptyString),
});
export type RuntimeCatalog = typeof RuntimeCatalog.Type;
