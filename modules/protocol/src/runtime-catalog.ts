import { HarnessId, ModelManifest } from "@artisan/catalog";
import { Schema } from "effect";

/** Describes the model and harness capabilities exposed by this Forge process. */
export const RuntimeCatalog = Schema.Struct({
	default_model_id: Schema.optionalKey(Schema.NonEmptyString),
	manifest: ModelManifest,
	/**
	 * Harnesses with an engine registered in this Forge process. The manifest
	 * always carries the full catalog so pickers can preview every model; a
	 * harness outside this list cannot start a session.
	 */
	runnable_harness_ids: Schema.Array(HarnessId),
});
export type RuntimeCatalog = typeof RuntimeCatalog.Type;
