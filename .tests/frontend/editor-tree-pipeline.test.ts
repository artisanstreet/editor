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
import {
	MergeWorkspaceEntries,
	WorkspaceEntriesByParent,
	workspace_tree_root,
	type WorkspaceTreeEntry,
} from "../../modules/frontend/src/lib/editor/workspace-session";

/**
 * The whole tree pipeline, end to end: the real backend walk feeding the real
 * frontend grouping. The unit tests either side of this seam both passed while
 * the tree still misbehaved in the app, so this exercises them together against
 * a repository shaped like the real one.
 */

const directories: Array<string> = [];

const make_repository = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-tree-"));
	directories.push(root);
	await mkdir(join(root, ".git", "objects"), { recursive: true });
	await writeFile(join(root, ".git", "objects", "pack"), "binary\n", "utf8");
	await mkdir(join(root, ".dist", "desktop"), { recursive: true });
	await writeFile(join(root, ".dist", "desktop", "main.js"), "// built\n", "utf8");
	await mkdir(join(root, "node_modules", "effect"), { recursive: true });
	await writeFile(join(root, "node_modules", "effect", "index.js"), "// vendored\n", "utf8");
	await mkdir(join(root, "modules", "frontend"), { recursive: true });
	await writeFile(join(root, "modules", "frontend", "app.ts"), "export {};\n", "utf8");
	await mkdir(join(root, "modules", "backend"), { recursive: true });
	await writeFile(join(root, ".gitignore"), ".dist\n", "utf8");
	await writeFile(join(root, "package.json"), "{}\n", "utf8");
	return root;
};

const Discover = (root: string, query: { readonly depth: number; readonly prefix?: string }) =>
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

/** Exactly what the sidebar does when a directory is opened. */
const LoadInto = async (
	tree: ReadonlyMap<string, ReadonlyArray<WorkspaceTreeEntry>>,
	root: string,
	parent: string,
) => {
	const page = await Discover(root, {
		depth: 1,
		...(parent === workspace_tree_root ? {} : { prefix: parent }),
	});

	return MergeWorkspaceEntries(tree, WorkspaceEntriesByParent(page.entries), parent);
};

afterAll(async () => {
	await Promise.all(
		directories.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("editor tree pipeline", () => {
	it("shows ignored and generated directories, hiding only .git", async () => {
		const root = await make_repository();
		const tree = await LoadInto(new Map(), root, workspace_tree_root);
		const names = tree.get(workspace_tree_root)?.map((entry) => entry.name) ?? [];

		expect(names).toContain(".dist");
		expect(names).toContain("node_modules");
		expect(names).toContain(".gitignore");
		expect(names).toContain("package.json");
		expect(names).not.toContain(".git");
	});

	it("lists one level, so the root costs one small page", async () => {
		const root = await make_repository();
		const tree = await LoadInto(new Map(), root, workspace_tree_root);

		expect(tree.has("modules")).toBe(false);
		expect(tree.get(workspace_tree_root)?.map((entry) => entry.name)).toEqual([
			".dist",
			"modules",
			"node_modules",
			".gitignore",
			"package.json",
		]);
	});

	/** The reported bug: expanding a folder must not remove its siblings. */
	it("keeps every other entry when a folder is expanded", async () => {
		const root = await make_repository();
		const opened = await LoadInto(
			await LoadInto(new Map(), root, workspace_tree_root),
			root,
			"modules",
		);

		expect(opened.get(workspace_tree_root)?.map((entry) => entry.name)).toEqual([
			".dist",
			"modules",
			"node_modules",
			".gitignore",
			"package.json",
		]);
		expect(opened.get("modules")?.map((entry) => entry.name)).toEqual(["backend", "frontend"]);
	});

	it("keeps earlier expansions when a second folder is opened", async () => {
		const root = await make_repository();
		const first = await LoadInto(
			await LoadInto(new Map(), root, workspace_tree_root),
			root,
			"modules",
		);
		const second = await LoadInto(first, root, ".dist");

		expect(second.get("modules")?.length).toBe(2);
		expect(second.get(".dist")?.map((entry) => entry.name)).toEqual(["desktop"]);
		expect(second.get(workspace_tree_root)?.length).toBe(5);
	});
});
