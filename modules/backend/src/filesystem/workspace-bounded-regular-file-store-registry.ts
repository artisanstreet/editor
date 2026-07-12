import { Context, Data, Effect, FileSystem, Layer, Redacted, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import {
	BoundedRegularFileStore,
	type BoundedRegularFileReader,
} from "./bounded-regular-file-store";
import {
	BuildNativeBoundedRegularFileStoreWithRootAuthorization,
	type NativeBoundedRegularFileStoreOptions,
} from "./native-bounded-regular-file-store";

const WorkspaceBoundedRegularFileStoreRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	workspace_id: Identifier,
});
const WorkspaceBoundedRegularFileStoreAuthorization = Schema.Struct({
	working_directory: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Supplies one native bounded regular-file root while retaining an opaque workspace identity. */
export type WorkspaceBoundedRegularFileStoreRegistration =
	typeof WorkspaceBoundedRegularFileStoreRegistration.Type;

/** Supplies the workspace and directory that must prove the same bounded-store root. */
export type WorkspaceBoundedRegularFileStoreAuthorization =
	typeof WorkspaceBoundedRegularFileStoreAuthorization.Type;

/** Reports malformed, missing, non-directory, duplicate, or aliased native store registration. */
export class WorkspaceBoundedRegularFileStoreRegistrationError extends Data.TaggedError(
	"WorkspaceBoundedRegularFileStoreRegistrationError",
)<{
	readonly message: string;
	readonly workspace_id?: string;
}> {}

/** Reports an opaque workspace identifier that has no registered native bounded regular-file store. */
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

/** Owns the opaque native bounded regular-file capabilities registered for known workspaces. */
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

/** Configures native store construction for every registration in one registry layer. */
export interface WorkspaceBoundedRegularFileStoreRegistryOptions {
	readonly load_native_module?: NativeBoundedRegularFileStoreOptions["load_native_module"];
	readonly receipt_authentication_key: Redacted.Redacted<Uint8Array>;
}

/** Supplies an inert registry when a portable backend has no native workspace roots. */
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

function registration_error(message: string, workspace_id?: string) {
	return new WorkspaceBoundedRegularFileStoreRegistrationError({
		...(workspace_id === undefined ? {} : { workspace_id }),
		message,
	});
}

function BuildWorkspaceBoundedRegularFileStoreRegistry(
	registrations: ReadonlyArray<unknown>,
	options: WorkspaceBoundedRegularFileStoreRegistryOptions,
) {
	return Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const decoded = yield* Effect.forEach(registrations, (registration) =>
			Schema.decodeUnknownEffect(WorkspaceBoundedRegularFileStoreRegistration, {
				onExcessProperty: "error",
			})(registration).pipe(
				Effect.mapError(() =>
					registration_error(
						"Workspace bounded regular-file store registration is invalid",
					),
				),
			),
		);
		const workspace_ids = decoded.map((registration) => registration.workspace_id);

		if (new Set(workspace_ids).size !== workspace_ids.length) {
			return yield* Effect.fail(
				registration_error(
					"Workspace bounded regular-file store registration IDs must be unique",
				),
			);
		}

		const canonical = yield* Effect.forEach(decoded, (registration) =>
			Effect.gen(function* () {
				const root = yield* file_system
					.realPath(registration.root)
					.pipe(
						Effect.mapError(() =>
							registration_error(
								"Workspace bounded regular-file store root could not be canonicalized",
								registration.workspace_id,
							),
						),
					);
				const entry = yield* file_system
					.stat(root)
					.pipe(
						Effect.mapError(() =>
							registration_error(
								"Workspace bounded regular-file store root could not be registered",
								registration.workspace_id,
							),
						),
					);

				if (entry.type !== "Directory") {
					return yield* Effect.fail(
						registration_error(
							"Workspace bounded regular-file store root must be a directory",
							registration.workspace_id,
						),
					);
				}

				return { root, workspace_id: registration.workspace_id };
			}),
		);

		if (new Set(canonical.map((registration) => registration.root)).size !== canonical.length) {
			return yield* Effect.fail(
				registration_error(
					"Workspace bounded regular-file store roots must be canonically unique",
				),
			);
		}

		const stores = yield* Effect.forEach(canonical, ({ root, workspace_id }) =>
			BuildNativeBoundedRegularFileStoreWithRootAuthorization({
				...(options.load_native_module === undefined
					? {}
					: { load_native_module: options.load_native_module }),
				receipt_authentication_key: options.receipt_authentication_key,
				root,
			}).pipe(
				Effect.map(({ AuthorizeRoot, store }) => ({ AuthorizeRoot, store, workspace_id })),
			),
		);
		const by_workspace_id = new Map(
			stores.map(({ store, workspace_id }) => [workspace_id, store] as const),
		);
		const readers_by_workspace_id = new Map(
			stores.map(
				({ store, workspace_id }) =>
					[
						workspace_id,
						{
							ReadRegularFile: store.ReadRegularFile,
						} satisfies BoundedRegularFileReader,
					] as const,
			),
		);
		const authorizers_by_workspace_id = new Map(
			stores.map(({ AuthorizeRoot, workspace_id }) => [workspace_id, AuthorizeRoot] as const),
		);
		const ListWorkspaceIds = Effect.succeed([...by_workspace_id.keys()].toSorted());
		const Get = (workspace_id: string) => {
			const reader = readers_by_workspace_id.get(workspace_id);

			return reader === undefined
				? Effect.fail(new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id }))
				: Effect.succeed({ reader, workspace_id });
		};
		const authorization_error = (workspace_id: string) =>
			new WorkspaceBoundedRegularFileStoreAuthorizationError({ workspace_id });
		const Authorize = (input: WorkspaceBoundedRegularFileStoreAuthorization) =>
			Schema.decodeUnknownEffect(WorkspaceBoundedRegularFileStoreAuthorization, {
				onExcessProperty: "error",
			})(input).pipe(
				Effect.mapError(() => authorization_error(input.workspace_id)),
				Effect.flatMap((authorization) =>
					Effect.gen(function* () {
						const AuthorizeRoot = authorizers_by_workspace_id.get(
							authorization.workspace_id,
						);
						const store = by_workspace_id.get(authorization.workspace_id);

						if (AuthorizeRoot === undefined || store === undefined) {
							return yield* Effect.fail(
								authorization_error(authorization.workspace_id),
							);
						}

						const authorized = yield* AuthorizeRoot(authorization.working_directory);

						if (!authorized) {
							return yield* Effect.fail(
								authorization_error(authorization.workspace_id),
							);
						}

						return { store, workspace_id: authorization.workspace_id };
					}),
				),
			);

		return { Authorize, Get, ListWorkspaceIds };
	});
}

/** Builds scoped native bounded regular-file capabilities from registered workspace roots. */
export function make_workspace_bounded_regular_file_store_registry_layer(
	registrations: ReadonlyArray<unknown>,
	options: WorkspaceBoundedRegularFileStoreRegistryOptions,
) {
	return Layer.effect(
		WorkspaceBoundedRegularFileStoreRegistry,
		BuildWorkspaceBoundedRegularFileStoreRegistry(registrations, options),
	);
}
