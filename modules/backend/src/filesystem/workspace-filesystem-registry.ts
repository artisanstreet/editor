import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Data, Effect, Layer, Schema } from "effect";

import { Identifier } from "@artisan/protocol";

import { Filesystem } from "./filesystem";
import { make_node_filesystem } from "./node-filesystem";

const WorkspaceFilesystemRegistration = Schema.Struct({
	root: Schema.NonEmptyString,
	watch_capacity: Schema.optional(Schema.Number),
	workspace_id: Identifier,
});

/** Supplies one backend-owned root while retaining the opaque workspace identity. */
export type WorkspaceFilesystemRegistration = typeof WorkspaceFilesystemRegistration.Type;

/** Exposes root-confined file operations without an absolute-path resolver. */
export type WorkspaceFilesystem = Omit<typeof Filesystem.Service, "Resolve">;

/** Reports malformed, missing, non-directory, duplicate, or aliased workspace registration. */
export class WorkspaceFilesystemRegistrationError extends Data.TaggedError(
	"WorkspaceFilesystemRegistrationError",
)<{
	readonly cause?: unknown;
	readonly message: string;
	readonly workspace_id?: string;
}> {}

/** Reports an opaque workspace identifier that has no registered filesystem capability. */
export class WorkspaceFilesystemNotFoundError extends Data.TaggedError(
	"WorkspaceFilesystemNotFoundError",
)<{
	readonly workspace_id: string;
}> {}

/** Owns the opaque backend filesystem capabilities registered for known workspaces. */
export class WorkspaceFilesystemRegistry extends Context.Service<
	WorkspaceFilesystemRegistry,
	{
		readonly Get: (
			workspace_id: string,
		) => Effect.Effect<
			{ readonly filesystem: WorkspaceFilesystem; readonly workspace_id: string },
			WorkspaceFilesystemNotFoundError
		>;
		readonly ListWorkspaceIds: Effect.Effect<ReadonlyArray<string>>;
	}
>()("Artisan/WorkspaceFilesystemRegistry") {}

function registration_error(message: string, cause?: unknown, workspace_id?: string) {
	return new WorkspaceFilesystemRegistrationError({
		...(cause === undefined ? {} : { cause }),
		...(workspace_id === undefined ? {} : { workspace_id }),
		message,
	});
}

function BuildWorkspaceFilesystemRegistry(registrations: ReadonlyArray<unknown>) {
	return Effect.gen(function* () {
		const decoded = yield* Effect.forEach(registrations, (registration) =>
			Schema.decodeUnknownEffect(WorkspaceFilesystemRegistration, {
				onExcessProperty: "error",
			})(registration).pipe(
				Effect.mapError((cause) =>
					registration_error("Workspace filesystem registration is invalid", cause),
				),
			),
		);
		const workspace_ids = decoded.map((registration) => registration.workspace_id);

		if (new Set(workspace_ids).size !== workspace_ids.length) {
			return yield* Effect.fail(
				registration_error("Workspace filesystem registration IDs must be unique"),
			);
		}

		const filesystems = yield* Effect.forEach(decoded, (registration) =>
			make_node_filesystem({
				root: registration.root,
				...(registration.watch_capacity === undefined
					? {}
					: { watch_capacity: registration.watch_capacity }),
			}).pipe(
				Effect.flatMap((filesystem) =>
					filesystem
						.Stat(".")
						.pipe(
							Effect.flatMap((entry) =>
								entry.kind === "directory"
									? Effect.succeed(filesystem)
									: Effect.fail(
											registration_error(
												"Workspace filesystem root must be a directory",
												undefined,
												registration.workspace_id,
											),
										),
							),
						),
				),
				Effect.mapError((cause) =>
					cause instanceof WorkspaceFilesystemRegistrationError
						? cause
						: registration_error(
								"Workspace filesystem root could not be registered",
								cause,
								registration.workspace_id,
							),
				),
				Effect.map((filesystem) => ({
					filesystem,
					workspace_id: registration.workspace_id,
				})),
			),
		);
		const roots = yield* Effect.forEach(filesystems, ({ filesystem, workspace_id }) =>
			filesystem.Resolve(".").pipe(
				Effect.map((root) => ({ root, workspace_id })),
				Effect.mapError((cause) =>
					registration_error(
						"Workspace filesystem root could not be canonicalized",
						cause,
						workspace_id,
					),
				),
			),
		);

		if (new Set(roots.map((entry) => entry.root)).size !== roots.length) {
			return yield* Effect.fail(
				registration_error("Workspace filesystem roots must be canonically unique"),
			);
		}

		const by_workspace_id = new Map(
			filesystems.map(({ filesystem, workspace_id }) => {
				const { Resolve: _, ...workspace_filesystem } = filesystem;

				return [workspace_id, workspace_filesystem] as const;
			}),
		);
		const ListWorkspaceIds = Effect.succeed([...by_workspace_id.keys()].toSorted());
		const Get = (workspace_id: string) => {
			const filesystem = by_workspace_id.get(workspace_id);

			return filesystem === undefined
				? Effect.fail(new WorkspaceFilesystemNotFoundError({ workspace_id }))
				: Effect.succeed({ filesystem, workspace_id });
		};

		return { Get, ListWorkspaceIds };
	});
}

/** Builds immutable backend-owned workspace filesystem capabilities from registered roots. */
export function make_node_workspace_filesystem_registry_layer(
	registrations: ReadonlyArray<unknown>,
) {
	return Layer.effect(
		WorkspaceFilesystemRegistry,
		BuildWorkspaceFilesystemRegistry(registrations),
	).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
		Layer.provideMerge(NodeCrypto.layer),
	);
}
