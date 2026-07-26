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

function make_service(root: string) {
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
	const layer = make_project_directory_service_layer([root]).pipe(
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
