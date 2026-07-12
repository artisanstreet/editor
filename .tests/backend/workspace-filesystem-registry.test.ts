import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	WorkspaceFilesystemRegistry,
	make_node_workspace_filesystem_registry_layer,
} from "../../modules/backend/src/filesystem/workspace-filesystem-registry";

const roots: Array<string> = [];

async function make_root(prefix = "artisan workspace registry ") {
	const root = await fs.mkdtemp(join(tmpdir(), prefix));

	roots.push(root);

	return root;
}

function make_registry(registrations: ReadonlyArray<unknown>) {
	return Effect.service(WorkspaceFilesystemRegistry).pipe(
		Effect.provide(make_node_workspace_filesystem_registry_layer(registrations)),
	);
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceFilesystemRegistry", () => {
	it("returns independent confined filesystem capabilities without exposing roots", async () => {
		const first_root = await make_root();
		const second_root = await make_root();

		await fs.writeFile(join(first_root, "first.txt"), "first");
		await fs.writeFile(join(second_root, "second.txt"), "second");

		const registry = await Effect.runPromise(
			make_registry([
				{ root: second_root, workspace_id: "workspace-b" },
				{ root: first_root, workspace_id: "workspace-a" },
			]),
		);
		const first = await Effect.runPromise(registry.Get("workspace-a"));
		const second = await Effect.runPromise(registry.Get("workspace-b"));

		expect(await Effect.runPromise(first.filesystem.ReadText("first.txt"))).toBe("first");
		expect(await Effect.runPromise(second.filesystem.ReadText("second.txt"))).toBe("second");
		expect(await Effect.runPromise(registry.ListWorkspaceIds)).toEqual([
			"workspace-a",
			"workspace-b",
		]);
		expect(Object.keys(first).toSorted()).toEqual(["filesystem", "workspace_id"]);
		expect("Resolve" in first.filesystem).toBe(false);
		expect("ReadRegularFile" in first.filesystem).toBe(false);
		expect("ReplaceRegularFile" in first.filesystem).toBe(false);
		expect("FinalizeRegularFileReplacement" in first.filesystem).toBe(false);
		expect(JSON.stringify(first)).not.toContain(first_root);
	});

	it("reports unknown workspace IDs", async () => {
		const registry = await Effect.runPromise(make_registry([]));
		const failure = await Effect.runPromise(registry.Get("missing").pipe(Effect.flip));

		expect(failure).toMatchObject({
			_tag: "WorkspaceFilesystemNotFoundError",
			workspace_id: "missing",
		});
	});

	it("authorizes only the registered canonical root without exposing it", async () => {
		const root = await make_root();
		const alias = join(tmpdir(), `artisan workspace authorization alias ${Date.now()}`);

		roots.push(alias);
		await fs.symlink(root, alias, "junction");

		const registry = await Effect.runPromise(
			make_registry([{ root, workspace_id: "workspace-a" }]),
		);
		const exact = await Effect.runPromise(
			registry.Authorize({ working_directory: root, workspace_id: "workspace-a" }),
		);
		const aliased = await Effect.runPromise(
			registry.Authorize({ working_directory: alias, workspace_id: "workspace-a" }),
		);

		expect(Object.keys(exact).toSorted()).toEqual(["filesystem", "workspace_id"]);
		expect("Resolve" in exact.filesystem).toBe(false);
		expect(aliased).toEqual(exact);
	});

	it("rejects unauthorized, missing, and non-directory working directories", async () => {
		const root = await make_root();
		const other_root = await make_root();
		const file = join(root, "file.txt");
		const missing = join(root, "missing");

		await fs.writeFile(file, "file");

		const registry = await Effect.runPromise(
			make_registry([{ root, workspace_id: "workspace-a" }]),
		);
		const failures = await Promise.all(
			[other_root, missing, file].map((working_directory) =>
				Effect.runPromise(
					registry
						.Authorize({ working_directory, workspace_id: "workspace-a" })
						.pipe(Effect.flip),
				),
			),
		);

		for (const failure of failures) {
			expect(failure).toMatchObject({
				_tag: "WorkspaceFilesystemAuthorizationError",
				workspace_id: "workspace-a",
			});
			expect(failure).not.toHaveProperty("cause");
			expect(JSON.stringify(failure)).not.toContain(root);
		}
	});

	it("keeps unknown workspace authorization opaque", async () => {
		const root = await make_root();
		const registry = await Effect.runPromise(make_registry([]));
		const failure = await Effect.runPromise(
			registry
				.Authorize({ working_directory: root, workspace_id: "missing" })
				.pipe(Effect.flip),
		);

		expect(failure).toMatchObject({
			_tag: "WorkspaceFilesystemNotFoundError",
			workspace_id: "missing",
		});
	});

	it("rejects duplicate IDs and canonical root aliases", async () => {
		const root = await make_root();
		const alias = join(tmpdir(), `artisan workspace alias ${Date.now()}`);

		roots.push(alias);
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

		expect(duplicate_id._tag).toBe("WorkspaceFilesystemRegistrationError");
		expect(duplicate_root._tag).toBe("WorkspaceFilesystemRegistrationError");
	});

	it("rejects missing, non-directory, and malformed registrations", async () => {
		const root = await make_root();
		const file = join(root, "file.txt");

		await fs.writeFile(file, "file");

		const missing = await Effect.runPromise(
			make_registry([{ root: join(root, "missing"), workspace_id: "workspace-a" }]).pipe(
				Effect.flip,
			),
		);
		const non_directory = await Effect.runPromise(
			make_registry([{ root: file, workspace_id: "workspace-a" }]).pipe(Effect.flip),
		);
		const malformed = await Effect.runPromise(
			make_registry([{ root, workspace_id: "" }]).pipe(Effect.flip),
		);

		expect(missing._tag).toBe("WorkspaceFilesystemRegistrationError");
		expect(non_directory._tag).toBe("WorkspaceFilesystemRegistrationError");
		expect(malformed._tag).toBe("WorkspaceFilesystemRegistrationError");
	});
});
