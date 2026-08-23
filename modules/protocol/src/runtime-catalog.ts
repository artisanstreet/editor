import { ModelManifest } from "@artisan/catalog";
import { Schema } from "effect";

/** Identifies the profile and location whose live provider inventory was read. */
export const RuntimeCatalogScope = Schema.Struct({
	profile_id: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
	workspace_trust: Schema.Literals(["safe", "trusted_project_config"]),
});
export type RuntimeCatalogScope = typeof RuntimeCatalogScope.Type;

/** Scoped provider route metadata normalized by the owning engine adapter. */
export const RuntimeCatalogRoute = Schema.Struct({
	engine_id: Schema.NonEmptyString,
	group: Schema.Struct({
		id: Schema.NonEmptyString,
		label: Schema.NonEmptyString,
		order: Schema.Int.check(Schema.isGreaterThanOrEqualTo(0)),
		show_route_labels: Schema.Boolean,
	}),
	id: Schema.NonEmptyString,
	label: Schema.NonEmptyString,
	status: Schema.Literals(["available", "unavailable"]),
	unavailable_reason: Schema.optional(Schema.NonEmptyString),
});
export type RuntimeCatalogRoute = typeof RuntimeCatalogRoute.Type;

/** Describes the model and harness capabilities exposed by this Forge process. */
export const RuntimeCatalog = Schema.Struct({
	/** Revision of the complete static-plus-live snapshot returned to the client. */
	catalog_revision: Schema.optionalKey(Schema.NonEmptyString),
	default_model_id: Schema.optionalKey(Schema.NonEmptyString),
	manifest: ModelManifest,
	/**
	 * Engines registered in this Forge process, by descriptor id. The manifest
	 * always carries the full catalog so pickers can preview every model; a
	 * harness outside this list cannot start a session. Ids stay unconstrained
	 * because registries may host engines outside the curated catalog.
	 */
	runnable_harness_ids: Schema.Array(Schema.NonEmptyString),
	/** Scoped execution routes; absent on catalogs produced before route normalization. */
	routes: Schema.optionalKey(Schema.Array(RuntimeCatalogRoute)),
	/** Present when at least one location/profile-scoped engine participated. */
	scope: Schema.optionalKey(RuntimeCatalogScope),
});
export type RuntimeCatalog = typeof RuntimeCatalog.Type;
