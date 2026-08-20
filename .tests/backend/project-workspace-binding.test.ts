import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Context, Deferred, Effect, Exit, Fiber, Layer, PubSub, Ref, Scope } from "effect";
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
import {
	make_node_workspace_git_registry_layer,
	WorkspaceGitRegistry,
} from "../../modules/backend/src/git/workspace-git-registry";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import {
	BindProjectWorkspaces,
	ProjectWorkspaceBindingError,
	ProjectWorkspaceBindingGate,
	ProjectWorkspaceBindingLive,
	make_project_workspace_binding_layer,
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

const ControlledCatalog = (
	projects: Ref.Ref<ProjectCatalogSnapshot["projects"]>,
	changes: PubSub.PubSub<ProjectCatalogSnapshot>,
) =>
	Layer.succeed(ProjectCatalog, {
		Attach: () => Effect.die("attach is not under test"),
		Detach: () => Effect.die("detach is not under test"),
		Find: () => Effect.die("find is not under test"),
		Snapshot: Ref.get(projects).pipe(Effect.map((projects) => ({ projects }))),
		Subscribe: PubSub.subscribe(changes),
	} as unknown as typeof ProjectCatalog.Service);

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
					yield* PubSub.publish(changes, { projects: [] }).pipe(Effect.forkScoped);
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
					yield* BindProjectWorkspaces();
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
						make_node_workspace_git_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		/** The workspace id is the project id, and the root is the project folder. */
		expect(registered.workspace_ids).toEqual(["project_1"]);
		expect(registered.entries).toContain("readme.md");
	});

	/**
	 * Git was the one workspace authority this binding never reconciled, so its
	 * registry held only what construction passed it — nothing, in production.
	 * Every workspace-scoped Git read failed "not found", which the surface showed
	 * by simply omitting its Changes row, and no Git projection was ever written
	 * for any workspace.
	 */
	it("registers an attached project's Git authority alongside its filesystem", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const registered = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces();
					const git = yield* WorkspaceGitRegistry;
					const capability = yield* git.Get("project_1");

					return {
						root: capability.git.root,
						workspace_ids: yield* git.ListWorkspaceIds,
					};
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						make_node_workspace_git_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(registered.workspace_ids).toEqual(["project_1"]);
		expect(registered.root).toContain("artisan-binding-");
	});

	/** Detaching must revoke Git authority, which is why the snapshot replaces. */
	it("revokes Git authority for a project that leaves the catalog", async () => {
		const first_root = await make_project_directory();
		const second_root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", first_root));
		const observed = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const binding = yield* BindProjectWorkspaces();
					const git = yield* WorkspaceGitRegistry;
					const before = yield* git.ListWorkspaceIds;

					yield* Ref.set(projects, [project("project_2", second_root)]);
					yield* binding.BindSnapshot;

					return {
						after: yield* git.ListWorkspaceIds,
						before,
						detached: (yield* Effect.exit(git.Get("project_1")))._tag,
					};
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						make_node_workspace_git_registry_layer([]),
						NodeWorkspaceBoundedRegularFileStoreRegistryLive,
					),
				),
			),
		);

		expect(observed.before).toEqual(["project_1"]);
		expect(observed.after).toEqual(["project_2"]);
		expect(observed.detached).toBe("Failure");
	});

	it("makes the project's files readable through the bounded store", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const content = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					yield* BindProjectWorkspaces();
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const workspace = yield* filesystems.Get("project_1");

					return yield* workspace.filesystem.ReadText("readme.md");
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						make_node_workspace_git_registry_layer([]),
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
					const binding = yield* BindProjectWorkspaces();
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
						make_node_workspace_git_registry_layer([]),
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
					yield* BindProjectWorkspaces();
					const filesystems = yield* WorkspaceFilesystemRegistry;

					return yield* filesystems.ListWorkspaceIds;
				}),
			).pipe(
				Effect.provide(
					Layer.mergeAll(
						StubCatalog(projects),
						make_node_workspace_filesystem_registry_layer([]),
						make_node_workspace_git_registry_layer([]),
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
					const binding = yield* BindProjectWorkspaces();
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
						make_node_workspace_git_registry_layer([]),
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
					const gate = yield* ProjectWorkspaceBindingGate;
					yield* gate.Await;
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
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result).toEqual({ bounded: [], filesystems: [] });
	});

	it("constructs immediately, shares one binding flight, and admits workspace work only after it succeeds", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const entered = Deferred.makeUnsafe<void>();
		const release = Deferred.makeUnsafe<void>();
		const reconciliations = Ref.makeUnsafe(0);

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					yield* Deferred.await(entered);
					const filesystems = yield* WorkspaceFilesystemRegistry;
					const waiters = yield* Effect.forEach(
						Array.from({ length: 8 }),
						() =>
							gate.Await.pipe(
								Effect.andThen(filesystems.Get("project_1")),
								Effect.map((workspace) => workspace.workspace_id),
								Effect.forkScoped,
							),
						{ concurrency: "unbounded" },
					);
					yield* Effect.forEach(Array.from({ length: 8 }), () => Effect.yieldNow, {
						discard: true,
					});
					const before_release = yield* Ref.get(reconciliations);
					yield* Deferred.succeed(release, undefined);
					return {
						before_release,
						ids: yield* Effect.forEach(waiters, (waiter) => Fiber.join(waiter)),
						reconciliations: yield* Ref.get(reconciliations),
					};
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.update(reconciliations, (count) => count + 1).pipe(
							Effect.andThen(Deferred.succeed(entered, undefined)),
							Effect.andThen(Deferred.await(release)),
						),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								StubCatalog(projects),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result).toEqual({
			before_release: 1,
			ids: Array.from({ length: 8 }, () => "project_1"),
			reconciliations: 1,
		});
	});

	it("publishes a typed binding failure and retries only when explicitly requested", async () => {
		const projects = ProjectsRef();
		const attempts = Ref.makeUnsafe(0);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					const first = yield* gate.Await.pipe(Effect.exit);
					yield* gate.Retry;
					const second = yield* gate.Await.pipe(Effect.exit);
					return { attempts: yield* Ref.get(attempts), first, second };
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(attempts, (count) => count + 1).pipe(
							Effect.flatMap((count) =>
								count === 1
									? Effect.fail(
											new ProjectWorkspaceBindingError({
												cause: "held failure",
												code: "reconcile_failed",
											}),
										)
									: Effect.void,
							),
						),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								StubCatalog(projects),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result.attempts).toBe(2);
		expect(Exit.isFailure(result.first)).toBe(true);
		expect(Exit.isSuccess(result.second)).toBe(true);
	});

	it("admits a claimed retry before an interrupted caller can leave it ownerless", async () => {
		const projects = ProjectsRef();
		const admitted = Deferred.makeUnsafe<void>();
		const release_admission = Deferred.makeUnsafe<void>();
		const retry_started = Deferred.makeUnsafe<void>();
		const attempts = Ref.makeUnsafe(0);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					yield* gate.Await.pipe(Effect.exit);
					const caller = yield* gate.Retry.pipe(Effect.forkScoped);
					yield* Deferred.await(admitted);
					yield* Fiber.interrupt(caller).pipe(Effect.forkScoped);
					yield* Deferred.succeed(release_admission, undefined);
					yield* Deferred.await(retry_started);
					return yield* gate.Await.pipe(Effect.exit);
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(attempts, (count) => count + 1).pipe(
							Effect.flatMap((count) =>
								count === 1
									? Effect.fail(
											new ProjectWorkspaceBindingError({
												cause: "initial failure",
												code: "reconcile_failed",
											}),
										)
									: Deferred.succeed(retry_started, undefined),
							),
						),
						OnRetryAdmission: Deferred.succeed(admitted, undefined).pipe(
							Effect.andThen(Deferred.await(release_admission)),
						),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								StubCatalog(projects),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(Exit.isSuccess(result)).toBe(true);
	});

	it("replaces authority admission for every catalog generation and shares one retry", async () => {
		const root = await make_project_directory();
		const projects = ProjectsRef(project("project_1", root));
		const held_change = Deferred.makeUnsafe<void>();
		const release_change = Deferred.makeUnsafe<void>();
		const held_retry = Deferred.makeUnsafe<void>();
		const release_retry = Deferred.makeUnsafe<void>();
		const entered_failed_change = Deferred.makeUnsafe<void>();
		const attempts = Ref.makeUnsafe(0);
		const changes = await Effect.runPromise(PubSub.unbounded<ProjectCatalogSnapshot>());

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					const filesystems = yield* WorkspaceFilesystemRegistry;
					yield* gate.Await;

					yield* Ref.set(projects, []);
					yield* PubSub.publish(changes, { projects: [] });
					yield* Deferred.await(held_change);
					const waiting =
						yield* Deferred.make<Exit.Exit<void, ProjectWorkspaceBindingError>>();
					yield* gate.Await.pipe(
						Effect.exit,
						Effect.tap((exit) => Deferred.succeed(waiting, exit)),
						Effect.forkScoped,
					);
					yield* Effect.yieldNow;
					const blocked = yield* Deferred.poll(waiting);
					yield* Deferred.succeed(release_change, undefined);
					yield* gate.Await;
					const detached = yield* filesystems.Get("project_1").pipe(Effect.exit);

					yield* Ref.set(projects, [project("project_1", root)]);
					yield* PubSub.publish(changes, { projects: yield* Ref.get(projects) }).pipe(
						Effect.forkScoped,
					);
					yield* Deferred.await(entered_failed_change);
					const failed = yield* gate.Await.pipe(Effect.exit);
					yield* Effect.all([gate.Retry, gate.Retry], { concurrency: "unbounded" });
					yield* Deferred.await(held_retry);
					yield* Deferred.succeed(release_retry, undefined);
					const retried = yield* gate.Await.pipe(Effect.exit);
					return {
						attempts: yield* Ref.get(attempts),
						blocked,
						detached,
						failed,
						retried,
					};
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(attempts, (count) => count + 1).pipe(
							Effect.flatMap((count) => {
								if (count === 2)
									return Deferred.succeed(held_change, undefined).pipe(
										Effect.andThen(Deferred.await(release_change)),
									);
								if (count === 3)
									return Deferred.succeed(entered_failed_change, undefined).pipe(
										Effect.andThen(
											Effect.fail(
												new ProjectWorkspaceBindingError({
													cause: "change failed",
													code: "reconcile_failed",
												}),
											),
										),
									);
								if (count === 4)
									return Deferred.succeed(held_retry, undefined).pipe(
										Effect.andThen(Deferred.await(release_retry)),
									);
								return Effect.void;
							}),
						),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								ControlledCatalog(projects, changes),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result.blocked._tag).toBe("None");
		expect(Exit.isFailure(result.detached)).toBe(true);
		expect(Exit.isFailure(result.failed)).toBe(true);
		expect(Exit.isSuccess(result.retried)).toBe(true);
		expect(result.attempts).toBe(4);
	});

	it("does not let an older rapid catalog generation settle the newer gate", async () => {
		const projects = ProjectsRef();
		const changes = await Effect.runPromise(PubSub.unbounded<ProjectCatalogSnapshot>());
		const first_entered = Deferred.makeUnsafe<void>();
		const first_release = Deferred.makeUnsafe<void>();
		const second_claimed = Deferred.makeUnsafe<void>();
		const second_entered = Deferred.makeUnsafe<void>();
		const second_release = Deferred.makeUnsafe<void>();
		const attempts = Ref.makeUnsafe(0);
		const active = Ref.makeUnsafe(0);
		const peak = Ref.makeUnsafe(0);
		const claims = Ref.makeUnsafe(0);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					yield* gate.Await;
					yield* PubSub.publish(changes, { projects: [] }).pipe(Effect.forkScoped);
					yield* Deferred.await(first_entered);
					const waiter =
						yield* Deferred.make<Exit.Exit<void, ProjectWorkspaceBindingError>>();
					yield* gate.Await.pipe(
						Effect.exit,
						Effect.tap((exit) => Deferred.succeed(waiter, exit)),
						Effect.forkScoped,
					);
					yield* PubSub.publish(changes, { projects: [] }).pipe(Effect.forkScoped);
					yield* Deferred.await(second_claimed);
					yield* Deferred.succeed(first_release, undefined);
					yield* Deferred.await(second_entered);
					yield* Effect.yieldNow;
					const before_second = yield* Deferred.poll(waiter);
					yield* Deferred.succeed(second_release, undefined);
					const after_second = yield* gate.Await.pipe(Effect.exit);
					return {
						after_second,
						attempts: yield* Ref.get(attempts),
						before_second,
						peak: yield* Ref.get(peak),
					};
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(active, (count) => count + 1).pipe(
							Effect.tap((count) =>
								Ref.update(peak, (current) => Math.max(current, count)),
							),
							Effect.andThen(Ref.updateAndGet(attempts, (count) => count + 1)),
							Effect.flatMap((count) =>
								(count === 2
									? Deferred.succeed(first_entered, undefined).pipe(
											Effect.andThen(Deferred.await(first_release)),
										)
									: count === 3
										? Deferred.succeed(second_entered, undefined).pipe(
												Effect.andThen(Deferred.await(second_release)),
											)
										: Effect.void
								).pipe(
									Effect.ensuring(Ref.update(active, (current) => current - 1)),
								),
							),
						),
						OnChangeClaimed: Ref.updateAndGet(claims, (count) => count + 1).pipe(
							Effect.flatMap((count) =>
								count === 2
									? Deferred.succeed(second_claimed, undefined)
									: Effect.void,
							),
						),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								ControlledCatalog(projects, changes),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result.attempts).toBe(3);
		expect(result.peak).toBe(1);
		expect(result.before_second._tag).toBe("None");
		expect(Exit.isSuccess(result.after_second)).toBe(true);
	});

	it("settles pre-claim waiters when scope close races a published catalog generation", async () => {
		const projects = ProjectsRef();
		const changes = await Effect.runPromise(PubSub.unbounded<ProjectCatalogSnapshot>());
		const initial_entered = Deferred.makeUnsafe<void>();
		const release_initial = Deferred.makeUnsafe<void>();
		const change_published = Deferred.makeUnsafe<void>();
		const release_claim = Deferred.makeUnsafe<void>();
		const base = Layer.mergeAll(
			ControlledCatalog(projects, changes),
			make_node_workspace_filesystem_registry_layer([]),
			make_node_workspace_git_registry_layer([]),
			NodeWorkspaceBoundedRegularFileStoreRegistryLive,
		);

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const scope = yield* Scope.make();
				const closer_scope = yield* Scope.make();
				const services = yield* Layer.build(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Deferred.succeed(initial_entered, undefined).pipe(
							Effect.andThen(Deferred.await(release_initial)),
						),
						OnChangePublished: Deferred.succeed(change_published, undefined).pipe(
							Effect.andThen(Deferred.await(release_claim)),
						),
					}).pipe(Layer.provideMerge(base)),
				).pipe(Effect.provideService(Scope.Scope, scope));
				const gate = Context.get(services, ProjectWorkspaceBindingGate);
				yield* Deferred.await(initial_entered);
				const waiter_scope = yield* Scope.make();
				const waiter = yield* Effect.forkIn(gate.Await.pipe(Effect.exit), waiter_scope);
				yield* Effect.yieldNow;
				yield* PubSub.publish(changes, { projects: [] });
				yield* Deferred.await(change_published);
				yield* Effect.forkIn(Scope.close(scope, Exit.void), closer_scope, {
					startImmediately: true,
				});
				yield* Effect.forEach(Array.from({ length: 8 }), () => Effect.yieldNow, {
					discard: true,
				});
				yield* Deferred.succeed(release_claim, undefined);
				yield* Deferred.succeed(release_initial, undefined);
				return yield* Fiber.join(waiter).pipe(Effect.timeoutOption("100 millis"));
			}),
		);

		expect(result._tag).toBe("Some");
		if (result._tag === "Some") expect(Exit.isFailure(result.value)).toBe(true);
	});

	it("runs ready operations concurrently and excludes them from claimed reconciliation", async () => {
		const projects = ProjectsRef();
		const changes = await Effect.runPromise(PubSub.unbounded<ProjectCatalogSnapshot>());
		const release_reads = Deferred.makeUnsafe<void>();
		const reads_entered = Deferred.makeUnsafe<void>();
		const change_claimed = Deferred.makeUnsafe<void>();
		const reconcile_entered = Deferred.makeUnsafe<void>();
		const release_reconcile = Deferred.makeUnsafe<void>();
		const post_claim_entered = Deferred.makeUnsafe<void>();
		const active_reads = Ref.makeUnsafe(0);
		const peak_reads = Ref.makeUnsafe(0);
		const reconciliations = Ref.makeUnsafe(0);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const gate = yield* ProjectWorkspaceBindingGate;
					yield* gate.Await;
					const Read = Ref.updateAndGet(active_reads, (count) => count + 1).pipe(
						Effect.tap((count) =>
							Ref.update(peak_reads, (peak) => Math.max(peak, count)),
						),
						Effect.tap((count) =>
							count === 2 ? Deferred.succeed(reads_entered, undefined) : Effect.void,
						),
						Effect.andThen(Deferred.await(release_reads)),
						Effect.ensuring(Ref.update(active_reads, (count) => count - 1)),
					);
					yield* Effect.all([gate.Use(Read), gate.Use(Read)], {
						concurrency: "unbounded",
					}).pipe(Effect.forkScoped);
					yield* Deferred.await(reads_entered);
					yield* PubSub.publish(changes, { projects: [] }).pipe(Effect.forkScoped);
					yield* Deferred.await(change_claimed);
					const post_claim = yield* gate
						.Use(Deferred.succeed(post_claim_entered, undefined))
						.pipe(Effect.forkScoped);
					yield* Effect.yieldNow;
					const reconcile_before_release = yield* Deferred.poll(reconcile_entered);
					const post_claim_before_release = yield* Deferred.poll(post_claim_entered);
					yield* Deferred.succeed(release_reads, undefined);
					yield* Deferred.await(reconcile_entered);
					yield* Effect.yieldNow;
					const post_claim_during_reconcile = yield* Deferred.poll(post_claim_entered);
					yield* Deferred.succeed(release_reconcile, undefined);
					yield* Fiber.join(post_claim);
					return {
						peak_reads: yield* Ref.get(peak_reads),
						post_claim_before_release,
						post_claim_during_reconcile,
						reconcile_before_release,
					};
				}),
			).pipe(
				Effect.provide(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(
							reconciliations,
							(count) => count + 1,
						).pipe(
							Effect.flatMap((count) =>
								count === 2
									? Deferred.succeed(reconcile_entered, undefined).pipe(
											Effect.andThen(Deferred.await(release_reconcile)),
										)
									: Effect.void,
							),
						),
						OnChangeClaimed: Deferred.succeed(change_claimed, undefined),
					}).pipe(
						Layer.provideMerge(
							Layer.mergeAll(
								ControlledCatalog(projects, changes),
								make_node_workspace_filesystem_registry_layer([]),
								make_node_workspace_git_registry_layer([]),
								NodeWorkspaceBoundedRegularFileStoreRegistryLive,
							),
						),
					),
				),
			),
		);

		expect(result.peak_reads).toBe(2);
		expect(result.reconcile_before_release._tag).toBe("None");
		expect(result.post_claim_before_release._tag).toBe("None");
		expect(result.post_claim_during_reconcile._tag).toBe("None");
	});

	it("interrupts held waiters when its scope closes and never reconciles after close", async () => {
		const projects = ProjectsRef();
		const entered = Deferred.makeUnsafe<void>();
		const release = Deferred.makeUnsafe<void>();
		const reconciliations = Ref.makeUnsafe(0);
		const base = Layer.mergeAll(
			StubCatalog(projects),
			make_node_workspace_filesystem_registry_layer([]),
			make_node_workspace_git_registry_layer([]),
			NodeWorkspaceBoundedRegularFileStoreRegistryLive,
		);

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const scope = yield* Scope.make();
				const services = yield* Layer.build(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.update(reconciliations, (count) => count + 1).pipe(
							Effect.andThen(Deferred.succeed(entered, undefined)),
							Effect.andThen(Deferred.await(release)),
						),
					}).pipe(Layer.provideMerge(base)),
				).pipe(Effect.provideService(Scope.Scope, scope));
				const gate = Context.get(services, ProjectWorkspaceBindingGate);
				const waiter_scope = yield* Scope.make();
				yield* Deferred.await(entered);
				const waiter = yield* Effect.forkIn(gate.Await.pipe(Effect.exit), waiter_scope);
				yield* Scope.close(scope, Exit.void);
				const waited = yield* Fiber.join(waiter);
				yield* Deferred.succeed(release, undefined);
				yield* Effect.yieldNow;
				return { reconciliations: yield* Ref.get(reconciliations), waited };
			}),
		);

		expect(result.reconciliations).toBe(1);
		expect(Exit.isFailure(result.waited)).toBe(true);
	});

	it("settles the current post-ready catalog generation when its service scope closes", async () => {
		const projects = ProjectsRef();
		const changes = await Effect.runPromise(PubSub.unbounded<ProjectCatalogSnapshot>());
		const entered = Deferred.makeUnsafe<void>();
		const release = Deferred.makeUnsafe<void>();
		const attempts = Ref.makeUnsafe(0);
		const base = Layer.mergeAll(
			ControlledCatalog(projects, changes),
			make_node_workspace_filesystem_registry_layer([]),
			make_node_workspace_git_registry_layer([]),
			NodeWorkspaceBoundedRegularFileStoreRegistryLive,
		);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const scope = yield* Scope.make();
				const services = yield* Layer.build(
					make_project_workspace_binding_layer({
						OnBeforeReconcile: Ref.updateAndGet(attempts, (count) => count + 1).pipe(
							Effect.flatMap((count) =>
								count === 2
									? Deferred.succeed(entered, undefined).pipe(
											Effect.andThen(Deferred.await(release)),
										)
									: Effect.void,
							),
						),
					}).pipe(Layer.provideMerge(base)),
				).pipe(Effect.provideService(Scope.Scope, scope));
				const gate = Context.get(services, ProjectWorkspaceBindingGate);
				yield* gate.Await;
				yield* PubSub.publish(changes, { projects: [] }).pipe(Effect.forkIn(scope));
				yield* Deferred.await(entered);
				const waiter_scope = yield* Scope.make();
				const waiter = yield* Effect.forkIn(gate.Await.pipe(Effect.exit), waiter_scope);
				yield* Scope.close(scope, Exit.void);
				const waited = yield* Fiber.join(waiter);
				yield* Deferred.succeed(release, undefined);
				yield* Effect.yieldNow;
				return { attempts: yield* Ref.get(attempts), waited };
			}),
		);

		expect(result.attempts).toBe(2);
		expect(Exit.isFailure(result.waited)).toBe(true);
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
					yield* BindProjectWorkspaces();
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
						make_node_workspace_git_registry_layer([]),
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
