import { Schema } from "effect";

import { Identifier } from "./common";
import { RichLinkAssetMetadata } from "./preview";
import { RepositoryHost } from "./repository";

/** One fetched provider image retained as a content-addressed asset. */
export const ProjectIdentityImage = Schema.Struct({
	...RichLinkAssetMetadata.fields,
	/** A repository's own image wins over the owner or organisation image. */
	source: Schema.Literals(["owner", "project"]),
});

export type ProjectIdentityImage = typeof ProjectIdentityImage.Type;

/**
 * The visual identity a project may safely present without exposing a remote
 * URL to the renderer. Local projects use the folder fallback; repositories
 * identify their provider and may carry a verified, retained provider image.
 */
export const ProjectIdentitySource = Schema.Union([
	Schema.Struct({ kind: Schema.Literal("folder"), project_id: Identifier }),
	Schema.Struct({
		host: RepositoryHost,
		image: Schema.optional(ProjectIdentityImage),
		kind: Schema.Literal("repository"),
		project_id: Identifier,
	}),
]);

export type ProjectIdentitySource = typeof ProjectIdentitySource.Type;

/** Exported so producers can keep batch requests and results inside the wire bound. */
export const ProjectIdentityMaximumProjects = 128;

/** Requests visual identities for named projects; empty asks for the attached catalog. */
export const ProjectIdentityQuery = Schema.Struct({
	project_ids: Schema.Array(Identifier).check(Schema.isMaxLength(ProjectIdentityMaximumProjects)),
});

export type ProjectIdentityQuery = typeof ProjectIdentityQuery.Type;

/** Returns one visual identity per requested project that Forge still owns. */
export const ProjectIdentityQueryResult = Schema.Struct({
	identities: Schema.Array(ProjectIdentitySource).check(
		Schema.isMaxLength(ProjectIdentityMaximumProjects),
	),
});

export type ProjectIdentityQueryResult = typeof ProjectIdentityQueryResult.Type;
