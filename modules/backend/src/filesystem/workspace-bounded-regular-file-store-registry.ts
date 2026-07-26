import { Context, Data, Effect, Layer, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import {
	BoundedRegularFileStore,
	type BoundedRegularFileReader,
} from "./bounded-regular-file-store";
const WorkspaceBoundedRegularFileStoreAuthorization = Schema.Struct({
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Supplies the workspace and directory that must prove the same bounded-store root. */
export type WorkspaceBoundedRegularFileStoreAuthorization =
	typeof WorkspaceBoundedRegularFileStoreAuthorization.Type;

/** Reports an opaque workspace identifier that has no registered bounded regular-file store. */
export class WorkspaceBoundedRegularFileStoreNotFoundError extends Data.TaggedError(
	"WorkspaceBoundedRegularFileStoreNotFoundError",
)<{
	readonly workspace_id: string;
}> {}

/** Reports a directory that cannot prove authority over its bounded regular-file store. */
export class WorkspaceBoundedRegularFileStoreAuthorizationError extends Data.TaggedError(
	"WorkspaceBoundedRegularFileStoreAuthorizationError",
)<{
	readonly workspace_id: string;
}> {}

/** Owns bounded regular-file capabilities supplied by an optional workspace authority. */
export class WorkspaceBoundedRegularFileStoreRegistry extends Context.Service<
	WorkspaceBoundedRegularFileStoreRegistry,
	{
		readonly Authorize: (input: WorkspaceBoundedRegularFileStoreAuthorization) => Effect.Effect<
			{
				readonly store: typeof BoundedRegularFileStore.Service;
				readonly workspace_id: string;
			},
			WorkspaceBoundedRegularFileStoreAuthorizationError
		>;
		readonly Get: (workspace_id: string) => Effect.Effect<
			{
				readonly reader: BoundedRegularFileReader;
				readonly workspace_id: string;
			},
			WorkspaceBoundedRegularFileStoreNotFoundError
		>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
	}
>()("Artisan/WorkspaceBoundedRegularFileStoreRegistry") {}

/** Supplies an inert registry when no external bounded-store authority is composed. */
export const EmptyWorkspaceBoundedRegularFileStoreRegistryLive = Layer.succeed(
	WorkspaceBoundedRegularFileStoreRegistry,
	{
		Authorize: ({ workspace_id }) =>
			Effect.fail(new WorkspaceBoundedRegularFileStoreAuthorizationError({ workspace_id })),
		Get: (workspace_id) =>
			Effect.fail(new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id })),
		ListWorkspaceIds: Effect.succeed([]),
	},
);
