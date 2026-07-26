import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { BoundedRegularFileStore } from "../../modules/backend/src/filesystem/bounded-regular-file-store";
import { WorkspaceBoundedRegularFileStoreRegistry } from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";
import {
	MakeCheckedTestWorkspaceBoundedRegularFileStoreRegistryLayer,
	TestWorkspaceBoundedRegularFileStoreRegistrationError,
} from "./bounded-regular-file-store-harness";

const roots: Array<string> = [];

const store: typeof BoundedRegularFileStore.Service = {
	FinalizeRegularFileReplacement: () => Effect.void,
	ReadRegularFile: () => Effect.succeed(new Uint8Array()),
	ReplaceRegularFile: () => Effect.succeed({ _tag: "Replaced" }),
};

const MakeRoot = async () => {
	const root = await fs.mkdtemp(join(tmpdir(), "artisan bounded registry "));

	roots.push(root);

	return root;
};

const Registry = (registrations: ReadonlyArray<{ root: string; workspace_id: string }>) =>
	Effect.service(WorkspaceBoundedRegularFileStoreRegistry).pipe(
		Effect.provide(
			MakeCheckedTestWorkspaceBoundedRegularFileStoreRegistryLayer(
				registrations.map((registration) => ({ ...registration, store })),
			),
		),
	);

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceBoundedRegularFileStoreRegistry", () => {
	it("canonicalizes roots and exposes sorted workspace IDs with read-only lookup", async () => {
		const first_root = await MakeRoot();
		const second_root = await MakeRoot();
		const registry = await Effect.runPromise(
			Registry([
				{ root: second_root, workspace_id: "workspace-b" },
				{ root: first_root, workspace_id: "workspace-a" },
			]),
		);
		const first = await Effect.runPromise(registry.Get("workspace-a"));

		expect(await Effect.runPromise(registry.ListWorkspaceIds)).toEqual([
			"workspace-a",
			"workspace-b",
		]);
		expect(Object.keys(first).toSorted()).toEqual(["reader", "workspace_id"]);
		expect(Object.keys(first.reader)).toEqual(["ReadRegularFile"]);
		expect("ReplaceRegularFile" in first.reader).toBe(false);
		expect("FinalizeRegularFileReplacement" in first.reader).toBe(false);
		expect(JSON.stringify(first)).not.toContain(first_root);
	});

	it("authorizes canonical aliases without exposing roots and rejects other roots", async () => {
		const root = await MakeRoot();
		const alias = join(tmpdir(), `artisan bounded registry alias ${Date.now()}`);

		roots.push(alias);
		await fs.symlink(root, alias, "junction");

		const registry = await Effect.runPromise(Registry([{ root, workspace_id: "workspace-a" }]));
		const authorized = await Effect.runPromise(
			registry.Authorize({ working_directory: alias, workspace_id: "workspace-a" }),
		);
		const wrong_root = await MakeRoot();
		const denied = await Effect.runPromise(
			registry
				.Authorize({ working_directory: wrong_root, workspace_id: "workspace-a" })
				.pipe(Effect.flip),
		);

		expect(Object.keys(authorized).toSorted()).toEqual(["store", "workspace_id"]);
		expect(JSON.stringify(authorized)).not.toContain(root);
		expect(denied).toMatchObject({
			_tag: "WorkspaceBoundedRegularFileStoreAuthorizationError",
			workspace_id: "workspace-a",
		});
		expect(JSON.stringify(denied)).not.toContain(wrong_root);
	});

	it("rejects duplicate IDs, duplicate canonical roots, and non-directory roots", async () => {
		const root = await MakeRoot();
		const other_root = await MakeRoot();
		const alias = join(tmpdir(), `artisan bounded registry duplicate ${Date.now()}`);
		const file = join(root, "file.txt");

		roots.push(alias);
		await fs.symlink(root, alias, "junction");
		await fs.writeFile(file, "file");

		for (const registrations of [
			[
				{ root, workspace_id: "workspace-a" },
				{ root: other_root, workspace_id: "workspace-a" },
			],
			[
				{ root, workspace_id: "workspace-a" },
				{ root: alias, workspace_id: "workspace-b" },
			],
			[{ root: file, workspace_id: "workspace-a" }],
		]) {
			const failure = await Effect.runPromise(Registry(registrations).pipe(Effect.flip));

			expect(failure).toBeInstanceOf(TestWorkspaceBoundedRegularFileStoreRegistrationError);
		}
	});

	it("keeps missing lookup errors opaque", async () => {
		const registry = await Effect.runPromise(Registry([]));
		const not_found = await Effect.runPromise(registry.Get("missing").pipe(Effect.flip));

		expect(not_found).toMatchObject({
			_tag: "WorkspaceBoundedRegularFileStoreNotFoundError",
			workspace_id: "missing",
		});
	});
});
