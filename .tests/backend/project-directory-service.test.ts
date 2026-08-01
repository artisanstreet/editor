import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_project_directory_service_layer,
	ProjectDirectoryService,
} from "../../modules/backend/src/projects/project-directory-service";
import { MakeSnowflakeIdLive } from "@artisan/protocol";
import { ProjectLocator } from "../../modules/backend/src/threads/project-locator";

const temporary_roots: Array<string> = [];

async function make_root(label: string) {
	const root = await fs.mkdtemp(join(tmpdir(), `artisan-${label}-`));
	temporary_roots.push(root);
	return fs.realpath(root);
}

function make_service(root: string, home_directory?: string) {
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
	const layer = make_project_directory_service_layer([root], home_directory).pipe(
		Layer.provideMerge(locator),
		Layer.provideMerge(MakeSnowflakeIdLive(37).pipe(Layer.orDie)),
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
	);
	return Effect.runPromise(Effect.service(ProjectDirectoryService).pipe(Effect.provide(layer)));
}

afterEach(async () => {
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
