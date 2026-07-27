import { ModelManifest } from "@artisan/catalog";
import { Schema } from "effect";

/** Describes the model and harness capabilities exposed by this Forge process. */
export const RuntimeCatalog = Schema.Struct({
	default_model_id: Schema.optionalKey(Schema.NonEmptyString),
	manifest: ModelManifest,
});
export type RuntimeCatalog = typeof RuntimeCatalog.Type;
