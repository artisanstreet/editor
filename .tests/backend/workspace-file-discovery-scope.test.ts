import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer } from "effect";
import { afterAll, describe, expect, it } from "vitest";

import { NodeWorkspaceBoundedRegularFileStoreRegistryLive } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import { make_node_workspace_filesystem_registry_layer } from "../../modules/backend/src/filesystem/workspace-filesystem-registry";
import {
	WorkspaceFileDiscovery,
	WorkspaceFileDiscoveryLive,
} from "../../modules/backend/src/workspace/files/discovery";

const directories: Array<string> = [];

/**
 * A repository shaped like a real one: generated output sorts before source,
 * which is exactly the case that made discovery return nothing but build
 * artifacts.
 */
const make_repository = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-discovery-"));
	directories.push(root);
	await mkdir(join(root, ".git", "objects"), { recursive: true });
	await writeFile(join(root, ".git", "objects", "pack"), "binary", "utf8");
	await mkdir(join(root, ".dist", "desktop"), { recursive: true });
	await writeFile(join(root, ".dist", "desktop", "main.js"), "// built\n", "utf8");
	await mkdir(join(root, "node_modules", "effect"), { recursive: true });
	await writeFile(join(root, "node_modules", "effect", "index.js"), "// vendored\n", "utf8");
	await mkdir(join(root, "modules", "frontend", "src"), { recursive: true });
	await writeFile(join(root, "modules", "frontend", "src", "app.ts"), "export {};\n", "utf8");
	await writeFile(join(root, "readme.md"), "# repo\n", "utf8");
	return root;
};

const Discover = (root: string, query: { readonly depth?: number; readonly prefix?: string }) =>
	Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const discovery = yield* WorkspaceFileDiscovery;

				return yield* discovery.Discover({
					limit: 1_000,
					workspace_id: "workspace",
					...query,
				});
			}),
		).pipe(
			Effect.provide(
				WorkspaceFileDiscoveryLive.pipe(
					Layer.provide(
						Layer.mergeAll(
							make_node_workspace_filesystem_registry_layer([
								{ root, workspace_id: "workspace" },
							]),
							NodeWorkspaceBoundedRegularFileStoreRegistryLive,
						),
					),
				),
			),
		),
	);

afterAll(async () => {
	await Promise.all(
		directories.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("workspace file discovery scope", () => {
	/** Ignored and generated trees are shown; only `.git` is refused. */
	it("shows generated and vendored directories", async () => {
		const root = await make_repository();
		const discovered = await Discover(root, { depth: 1 });
		const paths = discovered.entries.map((entry) => entry.path);

		expect(paths).toContain(".dist");
		expect(paths).toContain("node_modules");
		expect(paths).toContain("modules");
		expect(paths).toContain("readme.md");
	});

	it("refuses to walk .git", async () => {
		const root = await make_repository();
		const discovered = await Discover(root, {});
		const paths = discovered.entries.map((entry) => entry.path);

		expect(paths).not.toContain(".git");
		expect(paths.some((path) => path.startsWith(".git/"))).toBe(false);
	});

	it("returns one level when depth is 1", async () => {
		const root = await make_repository();
		const discovered = await Discover(root, { depth: 1 });

		expect(discovered.entries.map((entry) => entry.path).toSorted()).toEqual([
			".dist",
			"modules",
			"node_modules",
			"readme.md",
		]);
	});

	it("returns a directory's own children when given its prefix", async () => {
		const root = await make_repository();
		const discovered = await Discover(root, { depth: 1, prefix: "modules/frontend" });

		expect(discovered.entries.map((entry) => entry.path)).toContain("modules/frontend/src");
		expect(discovered.entries.map((entry) => entry.path)).not.toContain(
			"modules/frontend/src/app.ts",
		);
	});

	/** Absent depth keeps the previous unbounded behaviour for existing callers. */
	it("still walks the whole tree when depth is absent", async () => {
		const root = await make_repository();
		const discovered = await Discover(root, {});

		expect(discovered.entries.map((entry) => entry.path)).toContain(
			"modules/frontend/src/app.ts",
		);
	});
});
