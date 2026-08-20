import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, parse } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	Deferred,
	Effect,
	Exit,
	Fiber,
	FileSystem,
	Layer,
	ManagedRuntime,
	Option,
	Ref,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_project_directory_service_layer,
	ProjectDirectoryError,
	ProjectDirectoryService,
} from "../../modules/backend/src/projects/project-directory-service";
import { NativeDirectoryPicker } from "../../modules/backend/src/projects/native-directory-picker";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { ProjectLocator } from "../../modules/backend/src/threads/project-locator";

const temporary_roots: Array<string> = [];
const service_runtimes: Array<ManagedRuntime.ManagedRuntime<ProjectDirectoryService, never>> = [];

async function make_root(label: string) {
	const root = await fs.mkdtemp(join(tmpdir(), `artisan-${label}-`));
	temporary_roots.push(root);
	return fs.realpath(root);
}

function make_service(
	root: string,
	home_directory?: string,
	picked:
		| { readonly kind: "cancelled" }
		| { readonly kind: "selected"; readonly path: string } = {
		kind: "cancelled",
	},
	file_system = NodeFileSystem.layer,
	options?: Parameters<typeof make_project_directory_service_layer>[2],
) {
	const locator = Layer.succeed(ProjectLocator, {
		Locate: (location: string) =>
			Effect.succeed(
				Option.some({
					project: {
						display_name: basename(location),
						project_id: "project_test",
						root_path: location.replaceAll("\\", "/"),
					},
					source: "directory" as const,
				}),
			),
	});
	const layer = make_project_directory_service_layer([root], home_directory, options).pipe(
		Layer.provideMerge(locator),
		Layer.provideMerge(
			Layer.succeed(NativeDirectoryPicker, { Pick: () => Effect.succeed(picked) }),
		),
		Layer.provideMerge(MakeSnowflakeIdLive(37).pipe(Layer.orDie)),
		Layer.provideMerge(file_system),
		Layer.provideMerge(NodePath.layer),
	);
	const runtime = ManagedRuntime.make(layer);
	service_runtimes.push(runtime);
	return runtime.runPromise(Effect.service(ProjectDirectoryService));
}

const MakeBlockedProbeFileSystemLayer = (
	root: string,
	expected_probes: number,
	started: Deferred.Deferred<void>,
	release: Deferred.Deferred<void>,
	active: Ref.Ref<number>,
	peak: Ref.Ref<number>,
	probe_count: Ref.Ref<number>,
) =>
	Layer.effect(
		FileSystem.FileSystem,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;

			return {
				...file_system,
				stat: (path: string) =>
					path === root
						? file_system.stat(path)
						: Effect.gen(function* () {
								const now_active = yield* Ref.updateAndGet(
									active,
									(current) => current + 1,
								);
								yield* Ref.update(peak, (current) => Math.max(current, now_active));
								const count = yield* Ref.updateAndGet(
									probe_count,
									(current) => current + 1,
								);
								if (count === expected_probes) {
									yield* Deferred.succeed(started, undefined);
								}
								yield* Deferred.await(release);
								yield* Ref.update(active, (current) => current - 1);

								return yield* file_system.stat(path);
							}),
			};
		}),
	).pipe(Layer.provide(NodeFileSystem.layer));

const MakeHeldRootFileSystemLayer = (
	root: string,
	started: Deferred.Deferred<void>,
	release: Deferred.Deferred<void>,
	resolution_count: Ref.Ref<number>,
	lifecycle?: { interrupted: boolean },
) =>
	Layer.effect(
		FileSystem.FileSystem,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			return {
				...file_system,
				realPath: (path: string) =>
					path !== root
						? file_system.realPath(path)
						: Ref.updateAndGet(resolution_count, (count) => count + 1).pipe(
								Effect.tap((count) =>
									count === 1
										? Deferred.succeed(started, undefined)
										: Effect.void,
								),
								Effect.andThen(Deferred.await(release)),
								Effect.andThen(file_system.realPath(path)),
								Effect.onInterrupt(() =>
									Effect.sync(() => {
										if (lifecycle !== undefined) lifecycle.interrupted = true;
									}),
								),
							),
			};
		}),
	).pipe(Layer.provide(NodeFileSystem.layer));

const MakeBlockedStartupProbeFileSystemLayer = (
	targets: ReadonlySet<string>,
	expected_probes: number,
	started: Deferred.Deferred<void>,
	release: Deferred.Deferred<void>,
	active: Ref.Ref<number>,
	peak: Ref.Ref<number>,
	probe_count: Ref.Ref<number>,
) =>
	Layer.effect(
		FileSystem.FileSystem,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			return {
				...file_system,
				realPath: (path: string) =>
					!targets.has(path)
						? file_system.realPath(path)
						: Effect.gen(function* () {
								const now_active = yield* Ref.updateAndGet(
									active,
									(current) => current + 1,
								);
								yield* Ref.update(peak, (current) => Math.max(current, now_active));
								const count = yield* Ref.updateAndGet(
									probe_count,
									(current) => current + 1,
								);
								if (count === expected_probes) {
									yield* Deferred.succeed(started, undefined);
								}
								yield* Deferred.await(release);
								yield* Ref.update(active, (current) => current - 1);
								return yield* file_system.realPath(path);
							}),
			};
		}),
	).pipe(Layer.provide(NodeFileSystem.layer));

afterEach(async () => {
	await Promise.all(service_runtimes.splice(0).map((runtime) => runtime.dispose()));
	await Promise.all(
		temporary_roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })),
	);
});

describe("ProjectDirectoryService", () => {
	it("lists opaque directory ids and resolves a selected child through ProjectLocator", async () => {
		const root = await make_root("project-directory");
		const child = join(root, "workspace");
		await fs.mkdir(child);
		const service = await make_service(root);

		const roots = await Effect.runPromise(service.List({}));
		expect(roots.directories).toHaveLength(1);
		expect(roots.directories[0]).not.toHaveProperty("root_path");

		const children = await Effect.runPromise(
			service.List({ parent_directory_id: roots.directories[0]!.directory_id }),
		);
		expect(children.directories.map((entry) => entry.display_name)).toEqual(["workspace"]);

		const project = await Effect.runPromise(
			service.Select({ directory_id: children.directories[0]!.directory_id }),
		);
		expect(project).toMatchObject({ display_name: "workspace", project_id: "project_test" });
	});

	it("registers a native selection as a new opaque root and never returns its path", async () => {
		const root = await make_root("project-native-base");
		const selected = await make_root("project-native-selected");
		const service = await make_service(root, undefined, { kind: "selected", path: selected });

		const picked = await Effect.runPromise(service.Pick);
		expect(picked).toMatchObject({
			directory: { display_name: basename(selected), kind: "root" },
			status: "selected",
		});
		expect(JSON.stringify(picked)).not.toContain(selected);

		if (picked.status !== "selected") throw new Error("expected selected directory");
		const project = await Effect.runPromise(
			service.Select({ directory_id: picked.directory.directory_id }),
		);
		expect(project.root_path).toBe(selected.replaceAll("\\", "/"));
	});

	it("preserves native-picker cancellation without registering a directory", async () => {
		const root = await make_root("project-native-cancelled");
		const service = await make_service(root);

		await expect(Effect.runPromise(service.Pick)).resolves.toEqual({ status: "cancelled" });
	});

	it("does not disclose a selected filesystem root through its display label", async () => {
		const root = await make_root("project-native-volume-base");
		const volume_root = parse(root).root;
		const service = await make_service(root, undefined, {
			kind: "selected",
			path: volume_root,
		});

		const picked = await Effect.runPromise(service.Pick);
		expect(picked).toMatchObject({
			directory: { display_name: "Selected folder", kind: "root" },
			status: "selected",
		});
		expect(JSON.stringify(picked)).not.toContain(volume_root);
	});

	it("lists plain file names beside directories without making them selectable", async () => {
		const root = await make_root("project-files");
		await fs.mkdir(join(root, "workspace"));
		await fs.writeFile(join(root, "notes.md"), "notes");
		await fs.writeFile(join(root, "run.ps1"), "run");
		const service = await make_service(root);

		const roots = await Effect.runPromise(service.List({}));
		expect(roots.files).toEqual([]);

		const children = await Effect.runPromise(
			service.List({ parent_directory_id: roots.directories[0]!.directory_id }),
		);
		expect(children.directories.map((entry) => entry.display_name)).toEqual(["workspace"]);
		expect(children.files).toEqual(["notes.md", "run.ps1"]);
	});

	it("offers well-known home folders as places bounded by the allowed roots", async () => {
		const home = await make_root("project-home");
		await fs.mkdir(join(home, "Downloads"));
		await fs.mkdir(join(home, "Documents"));
		const service = await make_service(home, home);

		const listing = await Effect.runPromise(service.List({}));
		expect(listing.places?.map((place) => place.place)).toEqual([
			"home",
			"documents",
			"downloads",
		]);

		/** A place id is a first-class directory id: children list through it directly. */
		const downloads = listing.places?.find((place) => place.place === "downloads");
		const children = await Effect.runPromise(
			service.List({ parent_directory_id: downloads!.directory_id }),
		);
		expect(children.directories).toEqual([]);
	});

	it("creates a named folder inside a listed parent and rejects unsafe names", async () => {
		const root = await make_root("project-create");
		const service = await make_service(root);
		const roots = await Effect.runPromise(service.List({}));
		const parent_directory_id = roots.directories[0]!.directory_id;

		const created = await Effect.runPromise(
			service.Create({ name: "New folder", parent_directory_id }),
		);
		expect(created).toMatchObject({
			display_name: "New folder",
			has_children: false,
			kind: "directory",
		});

		const children = await Effect.runPromise(service.List({ parent_directory_id }));
		expect(children.directories.map((entry) => entry.display_name)).toEqual(["New folder"]);

		/** The created id is immediately selectable like any listed directory. */
		const project = await Effect.runPromise(
			service.Select({ directory_id: created.directory_id }),
		);
		expect(project.display_name).toBe("New folder");

		for (const name of ["..", "nested/child", "nested\\child", " padded "]) {
			await expect(
				Effect.runPromise(service.Create({ name, parent_directory_id })),
			).rejects.toThrow();
		}
		/** An existing folder is a caller error, never silently reused. */
		await expect(
			Effect.runPromise(service.Create({ name: "New folder", parent_directory_id })),
		).rejects.toThrow();
	});

	it("omits places that fall outside every allowed root", async () => {
		const root = await make_root("project-bounded");
		const home = await make_root("project-outside-home");
		await fs.mkdir(join(home, "Downloads"));
		const service = await make_service(root, home);

		const listing = await Effect.runPromise(service.List({}));
		expect(listing.places).toEqual([]);
	});

	it("publishes the layer before its scoped root initialization completes", async () => {
		const root = await make_root("project-held-layer");
		const controls = await Effect.runPromise(
			Effect.all({
				resolution_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const service = await make_service(
			root,
			undefined,
			{ kind: "cancelled" },
			MakeHeldRootFileSystemLayer(
				root,
				controls.started,
				controls.release,
				controls.resolution_count,
			),
		);

		/** `make_service` returned before this first filesystem operation can finish. */
		await Effect.runPromise(Deferred.await(controls.started));
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const listing = yield* service.List({}).pipe(Effect.forkScoped);
					yield* Effect.yieldNow;
					const resolution_count = yield* Ref.get(controls.resolution_count);
					yield* Deferred.succeed(controls.release, undefined);
					return { listing: yield* Fiber.join(listing), resolution_count };
				}),
			),
		);
		expect(result.resolution_count).toBe(1);
		expect(result.listing).toMatchObject({
			directories: [{ kind: "root" }],
		});
	});

	it("shares one initialization flight when many concurrent callers arrive before publication", async () => {
		const root = await make_root("project-shared-flight");
		const controls = await Effect.runPromise(
			Effect.all({
				resolution_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const service = await make_service(
			root,
			undefined,
			{ kind: "cancelled" },
			MakeHeldRootFileSystemLayer(
				root,
				controls.started,
				controls.release,
				controls.resolution_count,
			),
		);
		await Effect.runPromise(Deferred.await(controls.started));

		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const callers = yield* Effect.all(
						Array.from({ length: 12 }, () => service.List({})),
						{
							concurrency: "unbounded",
						},
					).pipe(Effect.forkScoped);
					yield* Effect.yieldNow;
					const resolution_count = yield* Ref.get(controls.resolution_count);
					yield* Deferred.succeed(controls.release, undefined);
					return { callers: yield* Fiber.join(callers), resolution_count };
				}),
			),
		);
		expect(result.resolution_count).toBe(1);
		expect(result.callers).toHaveLength(12);
		expect(result.callers).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					directories: [expect.objectContaining({ kind: "root" })],
				}),
			]),
		);
	});

	it("expires a typed initialization failure so a later directory call retries", async () => {
		const root = await make_root("project-initialization-retry");
		const attempts = await Effect.runPromise(Ref.make(0));
		const file_system = Layer.effect(
			FileSystem.FileSystem,
			Effect.gen(function* () {
				const node = yield* FileSystem.FileSystem;
				return {
					...node,
					realPath: (path) =>
						path !== root
							? node.realPath(path)
							: Ref.updateAndGet(attempts, (attempt) => attempt + 1).pipe(
									Effect.flatMap((attempt) =>
										attempt === 1
											? node.realPath(`${root}-temporarily-missing`)
											: node.realPath(path),
									),
								),
				};
			}),
		).pipe(Layer.provide(NodeFileSystem.layer));
		const service = await make_service(root, undefined, { kind: "cancelled" }, file_system);

		await expect(Effect.runPromise(service.List({}))).rejects.toBeInstanceOf(
			ProjectDirectoryError,
		);
		await expect(Effect.runPromise(service.List({}))).resolves.toMatchObject({
			directories: [{ kind: "root" }],
		});
		expect(await Effect.runPromise(Ref.get(attempts))).toBe(2);
	});

	it("shares one failed initialization with admitted callers before a later retry", async () => {
		const root = await make_root("project-shared-initialization-failure");
		const controls = await Effect.runPromise(
			Effect.all({
				attempts: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const file_system = Layer.effect(
			FileSystem.FileSystem,
			Effect.gen(function* () {
				const node = yield* FileSystem.FileSystem;
				return {
					...node,
					realPath: (path: string) =>
						path !== root
							? node.realPath(path)
							: Ref.updateAndGet(controls.attempts, (attempt) => attempt + 1).pipe(
									Effect.tap((attempt) =>
										attempt === 1
											? Deferred.succeed(controls.started, undefined).pipe(
													Effect.asVoid,
												)
											: Effect.void,
									),
									Effect.flatMap((attempt) =>
										attempt === 1
											? Deferred.await(controls.release).pipe(
													Effect.andThen(
														node.realPath(`${root}-missing`),
													),
												)
											: node.realPath(path),
									),
								),
				};
			}),
		).pipe(Layer.provide(NodeFileSystem.layer));
		const service = await make_service(root, undefined, { kind: "cancelled" }, file_system);
		await Effect.runPromise(Deferred.await(controls.started));

		const callers = Effect.runFork(
			Effect.all([service.List({}), service.Pick], { concurrency: "unbounded" }).pipe(
				Effect.exit,
			),
		);
		await Effect.runPromise(Deferred.succeed(controls.release, undefined));
		const result = await Effect.runPromise(Fiber.join(callers));

		expect(Exit.isFailure(result)).toBe(true);
		expect(await Effect.runPromise(Ref.get(controls.attempts))).toBe(1);
		await expect(Effect.runPromise(service.List({}))).resolves.toMatchObject({
			directories: [{ kind: "root" }],
		});
		expect(await Effect.runPromise(Ref.get(controls.attempts))).toBe(2);
	});

	it("keeps the scoped initialization flight alive when an admitted caller is interrupted", async () => {
		const root = await make_root("project-interrupted-caller");
		const controls = await Effect.runPromise(
			Effect.all({
				resolution_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const service = await make_service(
			root,
			undefined,
			{ kind: "cancelled" },
			MakeHeldRootFileSystemLayer(
				root,
				controls.started,
				controls.release,
				controls.resolution_count,
			),
		);
		await Effect.runPromise(Deferred.await(controls.started));
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const caller = yield* service.List({}).pipe(Effect.forkScoped);
					yield* Fiber.interrupt(caller);
				}),
			),
		);
		await Effect.runPromise(Deferred.succeed(controls.release, undefined));

		await expect(Effect.runPromise(service.List({}))).resolves.toMatchObject({
			directories: [{ kind: "root" }],
		});
		expect(await Effect.runPromise(Ref.get(controls.resolution_count))).toBe(1);
	});

	it("interrupts initialization on service scope close without late registry publication", async () => {
		const root = await make_root("project-initialization-scope-close");
		const controls = await Effect.runPromise(
			Effect.all({
				resolution_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const lifecycle = { interrupted: false };
		let registrations = 0;
		const service = await make_service(
			root,
			undefined,
			{ kind: "cancelled" },
			MakeHeldRootFileSystemLayer(
				root,
				controls.started,
				controls.release,
				controls.resolution_count,
				lifecycle,
			),
			{
				OnRegisterBeforePublish: () =>
					Effect.sync(() => {
						registrations += 1;
					}),
			},
		);
		await Effect.runPromise(Deferred.await(controls.started));
		const external_waiter = Effect.runFork(service.List({}));
		await Effect.runPromise(Effect.yieldNow);
		await service_runtimes.pop()!.dispose();
		await Effect.runPromise(Deferred.succeed(controls.release, undefined));
		const waiter_exit = await Effect.runPromise(Fiber.await(external_waiter));

		expect(lifecycle.interrupted).toBe(true);
		expect(registrations).toBe(0);
		expect(Exit.isFailure(waiter_exit)).toBe(true);
	});

	it("rejects retained callers after close when the prior failed flight has already cleared", async () => {
		const root = await make_root("project-post-close-admission");
		const attempts = await Effect.runPromise(Ref.make(0));
		const file_system = Layer.effect(
			FileSystem.FileSystem,
			Effect.gen(function* () {
				const node = yield* FileSystem.FileSystem;
				return {
					...node,
					realPath: (path: string) =>
						path !== root
							? node.realPath(path)
							: Ref.updateAndGet(attempts, (attempt) => attempt + 1).pipe(
									Effect.flatMap((attempt) =>
										attempt === 1
											? node.realPath(`${root}-missing`)
											: node.realPath(path),
									),
								),
				};
			}),
		).pipe(Layer.provide(NodeFileSystem.layer));
		const service = await make_service(root, undefined, { kind: "cancelled" }, file_system);
		await expect(Effect.runPromise(service.List({}))).rejects.toBeInstanceOf(
			ProjectDirectoryError,
		);
		expect(await Effect.runPromise(Ref.get(attempts))).toBe(1);

		await service_runtimes.pop()!.dispose();
		const callers = Effect.runFork(
			Effect.all([service.List({}), service.Pick], { concurrency: "unbounded" }).pipe(
				Effect.exit,
			),
		);
		const exit = await Effect.runPromise(Fiber.join(callers));

		expect(Exit.isFailure(exit)).toBe(true);
		expect(await Effect.runPromise(Ref.get(attempts))).toBe(1);
	});

	it("overlaps bounded configured-root and shortcut probes without changing presentation order", async () => {
		const root = await make_root("project-startup-probes");
		const place_names = ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"];
		await Promise.all(place_names.map((name) => fs.mkdir(join(root, name))));
		const expected_probes = 8;
		const controls = await Effect.runPromise(
			Effect.all({
				active: Ref.make(0),
				peak: Ref.make(0),
				probe_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const service = await make_service(
			root,
			root,
			{ kind: "cancelled" },
			MakeBlockedStartupProbeFileSystemLayer(
				new Set([root, ...place_names.map((name) => join(root, name))]),
				expected_probes,
				controls.started,
				controls.release,
				controls.active,
				controls.peak,
				controls.probe_count,
			),
		);
		await Effect.runPromise(Deferred.await(controls.started));
		const peak = await Effect.runPromise(Ref.get(controls.peak));
		await Effect.runPromise(Deferred.succeed(controls.release, undefined));
		const listing = await Effect.runPromise(service.List({}));
		const places = listing.places ?? [];

		expect(peak).toBeGreaterThan(1);
		expect(peak).toBeLessThanOrEqual(8);
		expect(places.map((place) => place.place)).toEqual([
			"home",
			"desktop",
			"documents",
			"downloads",
			"pictures",
			"music",
			"videos",
		]);
		const home = places[0]!;
		const root_entry = listing.directories[0]!;
		expect(home.directory_id).toBe(root_entry.directory_id);
	});

	it("bounds concurrent child probes, preserves order, and registers canonical aliases once", async () => {
		const root = await make_root("project-bounded-probes");
		const directories = [
			"zeta",
			"alpha",
			"theta",
			"beta",
			"delta",
			"gamma",
			"epsilon",
			"iota",
			"kappa",
			"lambda",
			"mu",
			"nu",
			"omicron",
			"pi",
			"rho",
			"sigma",
		];
		await Promise.all(directories.map((name) => fs.mkdir(join(root, name))));
		await fs.symlink(join(root, "alpha"), join(root, "alias-one"), "junction");
		await fs.symlink(join(root, "alpha"), join(root, "alias-two"), "junction");

		const probe_concurrency_bound = 16;
		const controls = await Effect.runPromise(
			Effect.all({
				active: Ref.make(0),
				peak: Ref.make(0),
				probe_count: Ref.make(0),
				release: Deferred.make<void>(),
				started: Deferred.make<void>(),
			}),
		);
		const service = await make_service(
			root,
			undefined,
			{ kind: "cancelled" },
			MakeBlockedProbeFileSystemLayer(
				root,
				probe_concurrency_bound,
				controls.started,
				controls.release,
				controls.active,
				controls.peak,
				controls.probe_count,
			),
		);
		const roots = await Effect.runPromise(service.List({}));

		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const listing = yield* service
					.List({ parent_directory_id: roots.directories[0]!.directory_id })
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(controls.started);
				const peak = yield* Ref.get(controls.peak);
				yield* Deferred.succeed(controls.release, undefined);

				return { listing: yield* Fiber.join(listing), peak };
			}),
		);

		expect(result.peak).toBeGreaterThan(1);
		expect(result.peak).toBeLessThanOrEqual(probe_concurrency_bound);
		expect(result.listing.directories.map((entry) => entry.display_name)).toEqual(
			[...directories, "alias-one", "alias-two"].toSorted((left, right) =>
				left.localeCompare(right),
			),
		);
		const aliases = result.listing.directories.filter((entry) =>
			entry.display_name.startsWith("alias-"),
		);
		expect(new Set(aliases.map((entry) => entry.directory_id)).size).toBe(1);
	});

	it("leaves no orphan id when registration is interrupted before its atomic publish", async () => {
		const root = await make_root("project-interrupted-registration");
		await fs.mkdir(join(root, "alpha"));
		await fs.symlink(join(root, "alpha"), join(root, "alias-one"), "junction");
		await fs.symlink(join(root, "alpha"), join(root, "alias-two"), "junction");
		const started = await Effect.runPromise(Deferred.make<void>());
		const release = await Effect.runPromise(Deferred.make<void>());
		let registrations = 0;
		let interrupted_id: string | undefined;
		const service = await make_service(root, undefined, { kind: "cancelled" }, undefined, {
			OnRegisterBeforePublish: ({ directory_id }) =>
				Effect.gen(function* () {
					registrations += 1;
					/** The root itself is the first registration; block the first child only. */
					if (registrations !== 2) return;
					interrupted_id = directory_id;
					yield* Deferred.succeed(started, undefined);
					yield* Deferred.await(release);
				}),
		});
		const roots = await Effect.runPromise(service.List({}));

		await Effect.runPromise(
			Effect.gen(function* () {
				const listing = yield* service
					.List({ parent_directory_id: roots.directories[0]!.directory_id })
					.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(started);
				yield* Fiber.interrupt(listing);
				yield* Deferred.succeed(release, undefined);
			}),
		);

		expect(interrupted_id).toBeDefined();
		await expect(
			Effect.runPromise(service.Select({ directory_id: interrupted_id! })),
		).rejects.toThrow();

		const retry = await Effect.runPromise(
			service.List({ parent_directory_id: roots.directories[0]!.directory_id }),
		);
		const aliases = retry.directories.filter((entry) =>
			["alpha", "alias-one", "alias-two"].includes(entry.display_name),
		);
		expect(aliases).toHaveLength(3);
		expect(new Set(aliases.map((entry) => entry.directory_id)).size).toBe(1);
	});

	it("does not expose a symlinked directory that escapes its allowed root", async () => {
		const root = await make_root("project-root");
		const outside = await make_root("project-outside");
		await fs.symlink(outside, join(root, "escape"), "junction");
		const service = await make_service(root);
		const roots = await Effect.runPromise(service.List({}));
		const children = await Effect.runPromise(
			service.List({ parent_directory_id: roots.directories[0]!.directory_id }),
		);

		expect(children.directories).toEqual([]);
	});
});
