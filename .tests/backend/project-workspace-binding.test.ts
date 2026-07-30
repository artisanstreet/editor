import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Deferred, Effect, Layer, PubSub, Ref } from "effect";
import { afterAll, describe, expect, it } from "vitest";

import type { ProjectCatalogSnapshot } from "@artisan/protocol";

import {
	NodeWorkspaceBoundedRegularFileStoreRegistryLive,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import {
	make_node_workspace_filesystem_registry_layer,
	WorkspaceFilesystemRegistry,
} from "../../modules/backend/src/filesystem/workspace-filesystem-registry";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import {
	BindProjectWorkspaces,
	ProjectWorkspaceBindingLive,
} from "../../modules/backend/src/workspace/projects";

const directories: Array<string> = [];

const make_project_directory = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-binding-"));
	directories.push(directory);
	await writeFile(join(directory, "readme.md"), "# project\n", "utf8");
	return directory;
};

/**
 * The catalog is stubbed to a fixed snapshot: this test is about the binding
 * between a project root and the workspace registries, not about persistence.
 */
const StubCatalog = (projects: Ref.Ref<ProjectCatalogSnapshot["projects"]>) =>
	Layer.effect(
		ProjectCatalog,
		Effect.gen(function* () {
			const changes = yield* PubSub.unbounded<ProjectCatalogSnapshot>();

			return {
				Attach: () => Effect.die("attach is not under test"),
				Detach: () => Effect.die("detach is not under test"),
				Find: () => Effect.die("find is not under test"),
				Snapshot: Ref.get(projects).pipe(Effect.map((projects) => ({ projects }))),
				Subscribe: PubSub.subscribe(changes),
			} as unknown as typeof ProjectCatalog.Service;
		}),
	);

const RacingDetachCatalog = (
	projects: Ref.Ref<ProjectCatalogSnapshot["projects"]>,
	reconciled: Deferred.Deferred<void>,
) =>
	Layer.effect(
		ProjectCatalog,
		Effect.gen(function* () {
			const changes = yield* PubSub.unbounded<ProjectCatalogSnapshot>();
			const first_snapshot = yield* Ref.make(true);
			const Snapshot = Effect.gen(function* () {
				const current = yield* Ref.get(projects);
				const race = yield* Ref.getAndSet(first_snapshot, false);
				if (race) {
					yield* Ref.set(projects, []);
					yield* PubSub.publish(changes, { projects: [] });
				} else {
					yield* Deferred.succeed(reconciled, undefined);
				}
				return { projects: current };
			});
			return {
				Attach: () => Effect.die("attach is not under test"),
				Detach: () => Effect.die("detach is not under test"),
				Find: () => Effect.die("find is not under test"),
				Snapshot,
				Subscribe: PubSub.subscribe(changes),
			} as unknown as typeof ProjectCatalog.Service;
		}),
	);

const project = (project_id: string, root_path: string) => ({
	attached_at: "2026-07-29T00:00:00.000Z",
	display_name: project_id,
	project_id,
	root_path,
	updated_at: "2026-07-29T00:00:00.000Z",
});

const ProjectsRef = (...projects: ProjectCatalogSnapshot["projects"]) =>
	Ref.makeUnsafe<ProjectCatalogSnapshot["projects"]>(projects);

afterAll(async () => {
	await Promise.all(
		directories.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("project workspace binding", () => {
	it("registers an attached project's own directory as its workspace", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const registered = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces;
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const workspace_ids = yield* filesystems.ListWorkspaceIds;
					const workspace = yield* filesystems.Get("project_1");
					const entries = yield* workspace.filesystem.List(".");

					return {
						entries: entries.map((entry) => entry.path),
						workspace_ids,
					};
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		/** The workspace id is the project id, and the root is the project folder. */
		expect(registered.workspace_ids).toEqual(["project_1"]);
		expect(registered.entries).toContain("readme.md");
	});

	it("makes the project's files readable through the bounded store", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const content = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces;
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const workspace = yield* filesystems.Get("project_1");

					return yield* workspace.filesystem.ReadText("readme.md");
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(content).toContain("# project");
	});

	/** Re-running the bind over an already-bound catalog must not fail. */
	it("binds idempotently so a startup replay is safe", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const workspace_ids = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const binding = yield* BindProjectWorkspaces;
					yield* binding.BindSnapshot;
					yield* binding.BindSnapshot;
					const filesystems = yield* WorkspaceFilesystemRegistry;

					return yield* filesystems.ListWorkspaceIds;
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(workspace_ids).toEqual(["project_1"]);
	});

	/** One unreachable root must not stop the other projects from binding. */
	it("skips a project whose directory is missing", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(
			project("project_missing", join(root, "absent")),
			project("project_present", root),
		);
		const workspace_ids = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces;
					const filesystems = yield* WorkspaceFilesystemRegistry;

					return yield* filesystems.ListWorkspaceIds;
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(workspace_ids).toEqual(["project_present"]);
	});

	it("revokes detached capabilities and permits a later reattach", async () => {
		const first_root = await make_project_directory();
		const second_root = await make_project_directory();
		await writeFile(join(second_root, "readme.md"), "# reattached\n", "utf8");
		const projects = ProjectsRef(project("project_1", first_root));

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const binding = yield* BindProjectWorkspaces;
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const stores = yield* WorkspaceBoundedRegularFileStoreRegistry;

					yield* Ref.set(projects, []);
					yield* binding.BindSnapshot;

					const detached_tree = yield* filesystems.Get("project_1").pipe(Effect.result);
					const detached_read = yield* stores.Get("project_1").pipe(Effect.result);
					const detached_write = yield* stores
						.Authorize({
							working_directory: first_root,
							workspace_id: "project_1",
						})
						.pipe(Effect.result);

					yield* Ref.set(projects, [project("project_1", second_root)]);
					yield* binding.BindSnapshot;
					const reattached = yield* filesystems.Get("project_1");
					const content = yield* reattached.filesystem.ReadText("readme.md");

					return {
						content,
						detached_read: detached_read._tag,
						detached_tree: detached_tree._tag,
						detached_write: detached_write._tag,
						workspace_ids: yield* filesystems.ListWorkspaceIds,
					};
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(result).toEqual({
			content: "# reattached\n",
			detached_read: "Failure",
			detached_tree: "Failure",
			detached_write: "Failure",
			workspace_ids: ["project_1"],
		});
	});

	it("does not miss a detach racing the startup snapshot", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const reconciled = Deferred.makeUnsafe<void>();

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* Deferred.await(reconciled);
					yield* Effect.forEach(Array.from({ length: 8 }), () => Effect.yieldNow, {
						discard: true,
					});
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const stores = yield* WorkspaceBoundedRegularFileStoreRegistry;
					return {
						bounded: yield* stores.ListWorkspaceIds,
						filesystems: yield* filesystems.ListWorkspaceIds,
					};
				}),
			).pipe(
				Effect.provide(
					ProjectWorkspaceBindingLive.pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								RacingDetachCatalog(projects, reconciled),
								make_node_workspace_filesystem_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result).toEqual({ bounded: [], filesystems: [] });
	});

	it("propagates only accepted filesystem authorities to the bounded store", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(
			project("project_1", root),
			project("project_duplicate_root", root),
		);

		const workspace_ids = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces;
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const stores = yield* WorkspaceBoundedRegularFileStoreRegistry;

					return {
						bounded: yield* stores.ListWorkspaceIds,
						filesystems: yield* filesystems.ListWorkspaceIds,
					};
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(workspace_ids).toEqual({
			bounded: ["project_1"],
			filesystems: ["project_1"],
		});
	});
});
