import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	WorkspaceGitRegistry,
	WorkspaceGitRegistrationError,
	WorkspaceGitNotFoundError,
	make_node_workspace_git_registry_layer,
} from "../../modules/backend/src/git/workspace-git-registry";

const roots: Array<string> = [];

async function make_root(prefix = "artisan workspace git registry ") {
	const root = await fs.mkdtemp(join(tmpdir(), prefix));

	roots.push(root);

	return root;
}

function make_registry(registrations: ReadonlyArray<unknown>) {
	return Effect.service(WorkspaceGitRegistry).pipe(
		Effect.provide(make_node_workspace_git_registry_layer(registrations)),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceGitRegistry", () => {
	it("validates registrations and canonicalizes roots before building capabilities", async () => {
		const root = await make_root();
		const file = join(root, "file.txt");
		const alias = join(tmpdir(), `artisan workspace git alias ${Date.now()}`);

		roots.push(alias);
		await fs.writeFile(file, "file");
		await fs.symlink(root, alias, "junction");

		const duplicate_id = await Effect.runPromise(
			make_registry([
				{ root, workspace_id: "workspace-a" },
				{ root: await make_root(), workspace_id: "workspace-a" },
			]).pipe(Effect.flip),
		);
		const duplicate_root = await Effect.runPromise(
			make_registry([
				{ root, workspace_id: "workspace-a" },
				{ root: alias, workspace_id: "workspace-b" },
			]).pipe(Effect.flip),
		);
		const invalid = await Effect.runPromise(
			make_registry([{ root: file, workspace_id: "workspace-a" }]).pipe(Effect.flip),
		);

		expect(duplicate_id).toBeInstanceOf(WorkspaceGitRegistrationError);
		expect(duplicate_root).toBeInstanceOf(WorkspaceGitRegistrationError);
		expect(invalid).toBeInstanceOf(WorkspaceGitRegistrationError);
	});

	it("returns sorted opaque IDs and reports an unknown workspace", async () => {
		const first_root = await make_root();
		const second_root = await make_root();
		const registry = await Effect.runPromise(
			make_registry([
				{ root: second_root, workspace_id: "workspace-b" },
				{ root: first_root, workspace_id: "workspace-a" },
			]),
		);
		const first = await Effect.runPromise(registry.Get("workspace-a"));
		const missing = await Effect.runPromise(registry.Get("missing").pipe(Effect.flip));

		expect(await Effect.runPromise(registry.ListWorkspaceIds)).toEqual([
			"workspace-a",
			"workspace-b",
		]);
		expect(first.workspace_id).toBe("workspace-a");
		expect(first.canonical_root).toBe(await fs.realpath(first_root));
		expect(missing).toBeInstanceOf(WorkspaceGitNotFoundError);
		expect(missing).toMatchObject({ workspace_id: "missing" });
	});
});
