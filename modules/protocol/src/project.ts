import { Schema } from "effect";

import { Identifier, IsoDateTime } from "./common";

/** Describes one project attached to and owned by Forge. */
export const Project = Schema.Struct({
	attached_at: IsoDateTime,
	display_name: Schema.NonEmptyString,
	project_id: Identifier,
	root_path: Schema.NonEmptyString,
	updated_at: IsoDateTime,
});

export type Project = typeof Project.Type;

/** Projects the complete authoritative Forge project catalog. */
export const ProjectCatalogSnapshot = Schema.Struct({
	projects: Schema.Array(Project),
});

export type ProjectCatalogSnapshot = typeof ProjectCatalogSnapshot.Type;

/** Detaches a Forge-owned project by identity without accepting client path data. */
export const ProjectDetachInput = Schema.Struct({
	project_id: Identifier,
});

export type ProjectDetachInput = typeof ProjectDetachInput.Type;
