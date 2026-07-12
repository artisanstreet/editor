import { Context, Data, Effect, FileSystem, Layer, Redacted, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import { BoundedRegularFileStore } from "./bounded-regular-file-store";
import {
	BuildNativeBoundedRegularFileStore,
	type NativeBoundedRegularFileStoreOptions,
} from "./native-bounded-regular-file-store";

const WorkspaceBoundedRegularFileStoreRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	workspace_id: Identifier,
});

/** Supplies one native bounded regular-file root while retaining an opaque workspace identity. */
export type WorkspaceBoundedRegularFileStoreRegistration =
	typeof WorkspaceBoundedRegularFileStoreRegistration.Type;

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

/** Owns the opaque native bounded regular-file capabilities registered for known workspaces. */
export class WorkspaceBoundedRegularFileStoreRegistry extends Context.Service<
	WorkspaceBoundedRegularFileStoreRegistry,
	{
		readonly Get: (workspace_id: string) => Effect.Effect<
			{
				readonly store: typeof BoundedRegularFileStore.Service;
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
			BuildNativeBoundedRegularFileStore({
				...(options.load_native_module === undefined
					? {}
					: { load_native_module: options.load_native_module }),
				receipt_authentication_key: options.receipt_authentication_key,
				root,
			}).pipe(Effect.map((store) => ({ store, workspace_id }))),
		);
		const by_workspace_id = new Map(
			stores.map(({ store, workspace_id }) => [workspace_id, store] as const),
		);
		const ListWorkspaceIds = Effect.succeed([...by_workspace_id.keys()].toSorted());
		const Get = (workspace_id: string) => {
			const store = by_workspace_id.get(workspace_id);

			return store === undefined
				? Effect.fail(new WorkspaceBoundedRegularFileStoreNotFoundError({ workspace_id }))
				: Effect.succeed({ store, workspace_id });
		};

		return { Get, ListWorkspaceIds };
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
