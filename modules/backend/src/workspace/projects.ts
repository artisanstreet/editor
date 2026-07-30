import { Effect, Layer, PubSub } from "effect";

import { WorkspaceBoundedRegularFileStoreRegistry } from "../filesystem/workspace-bounded-regular-file-store-registry";
import { WorkspaceFilesystemRegistry } from "../filesystem/workspace-filesystem-registry";
import { ProjectCatalog } from "../projects/project-catalog";

/**
 * Binds every attached project to a workspace over its own directory.
 *
 * A project is a folder the user attached, and the workspace the protocol reads
 * is that same folder — so the project id is the workspace id and the project
 * root is the workspace root. Without this the workspace registries stay empty
 * and every file query answers "workspace not found" for a directory Forge is
 * already showing in its own sidebar.
 *
 * Binding is idempotent and runs in two places: once over the catalog at
 * startup, and again for every catalog change, so a project attached while
 * Forge runs is browsable immediately rather than after a restart.
 */
export const BindProjectWorkspaces = Effect.gen(function* () {
	const catalog = yield* ProjectCatalog;
	const filesystems = yield* WorkspaceFilesystemRegistry;
	const stores = yield* WorkspaceBoundedRegularFileStoreRegistry;

	const BindSnapshot = catalog.Snapshot.pipe(
		Effect.flatMap((snapshot) =>
			filesystems
				.Reconcile(
					snapshot.projects.map((project) => ({
						root: project.root_path,
						workspace_id: project.project_id,
					})),
				)
				.pipe(Effect.flatMap(stores.Reconcile)),
		),
		Effect.catchCause((cause) =>
			Effect.logWarning("Project catalog could not be read for workspace binding", { cause }),
		),
	);

	yield* BindSnapshot;

	return { BindSnapshot };
});

/**
 * Keeps the binding current for the life of the process. The subscription is
 * forked into the layer's scope, so it ends with the backend rather than
 * outliving it.
 */
export const ProjectWorkspaceBindingLive = Layer.effectDiscard(
	Effect.gen(function* () {
		const catalog = yield* ProjectCatalog;
		const changes = yield* catalog.Subscribe;
		/**
		 * Subscribe before the authoritative startup snapshot. A catalog mutation
		 * racing that snapshot is then queued here and forces a second reconcile,
		 * so neither attachment nor revocation can disappear in the gap.
		 */
		const binding = yield* BindProjectWorkspaces;

		yield* PubSub.take(changes).pipe(
			Effect.flatMap(() => binding.BindSnapshot),
			Effect.forever,
			Effect.forkScoped,
		);
	}),
);
