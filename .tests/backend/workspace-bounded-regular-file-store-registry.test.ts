import { promises as fs } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem } from "@effect/platform-node-shared";
import { Effect, Layer, Redacted } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	make_workspace_bounded_regular_file_store_registry_layer,
	WorkspaceBoundedRegularFileStoreRegistry,
} from "../../modules/backend/src/filesystem/workspace-bounded-regular-file-store-registry";

const roots: Array<string> = [];
const receipt_authentication_key = Redacted.make(new Uint8Array(32).fill(3));

async function make_root(prefix = "artisan native registry ") {
	const root = await fs.mkdtemp(join(tmpdir(), prefix));

	roots.push(root);

	return root;
}

function make_module(throw_on_root?: string) {
	const constructed_roots: Array<string> = [];
	const authorized_roots = new Set<string>();
	let close_count = 0;
	let load_count = 0;

	class FakeNativeBoundedRegularFileStore {
		constructor(root: string, _key: Uint8Array) {
			constructed_roots.push(root);
			authorized_roots.add(root);

			if (root === throw_on_root) throw new Error("open failed");
		}

		close() {
			close_count += 1;
		}

		authorizeRoot(candidate_root: string) {
			return Promise.resolve(authorized_roots.has(candidate_root));
		}

		finalizeRegularFileReplacement() {
			return Promise.resolve();
		}

		readRegularFile() {
			return Promise.resolve(new Uint8Array());
		}

		replaceRegularFile() {
			return Promise.resolve("Replaced");
		}
	}

	return {
		authorized_roots,
		constructed_roots,
		get close_count() {
			return close_count;
		},
		get load_count() {
			return load_count;
		},
		load_native_module: () => {
			load_count += 1;

			return {
				NativeBoundedRegularFileStore: FakeNativeBoundedRegularFileStore,
				getNativeBuildDescriptor: () => ({
					architecture: "x86_64",
					operatingSystem: "windows",
					target: "x86_64-pc-windows-msvc",
					testHooksEnabled: false,
				}),
			};
		},
	};
}

function registry_effect(registrations: ReadonlyArray<unknown>, module = make_module()) {
	return {
		effect: Effect.scoped(
			Effect.service(WorkspaceBoundedRegularFileStoreRegistry).pipe(
				Effect.provide(
					make_workspace_bounded_regular_file_store_registry_layer(registrations, {
						load_native_module: module.load_native_module,
						receipt_authentication_key,
					}).pipe(Layer.provide(NodeFileSystem.layer)),
				),
			),
		),
		module,
	};
}

afterEach(async () => {
	await Promise.all(roots.splice(0).map((root) => fs.rm(root, { force: true, recursive: true })));
});

describe("WorkspaceBoundedRegularFileStoreRegistry", () => {
	it("canonicalizes roots and exposes only workspace IDs with bounded stores", async () => {
		const first_root = await make_root();
		const second_root = await make_root();
		const module = make_module();
		const registry = await Effect.runPromise(
			registry_effect(
				[
					{ root: second_root, workspace_id: "workspace-b" },
					{ root: first_root, workspace_id: "workspace-a" },
				],
				module,
			).effect,
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
		expect(module.constructed_roots).toEqual([second_root, first_root]);
	});

	it("authorizes only the canonical registered root without exposing it", async () => {
		const root = await make_root();
		const alias = join(tmpdir(), `artisan native registry authorize ${Date.now()}`);

		roots.push(alias);
		await fs.symlink(root, alias, "junction");
		const module = make_module();

		module.authorized_roots.add(alias);

		const registry = await Effect.runPromise(
			registry_effect([{ root, workspace_id: "workspace-a" }], module).effect,
		);
		const authorized = await Effect.runPromise(
			registry.Authorize({ working_directory: alias, workspace_id: "workspace-a" }),
		);
		const wrong_root = await make_root();
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

	it("fails closed when a registered root is replaced at the same path", async () => {
		const root = await make_root();
		const module = make_module();
		const registry = await Effect.runPromise(
			registry_effect([{ root, workspace_id: "workspace-a" }], module).effect,
		);

		module.authorized_roots.delete(root);

		const denied = await Effect.runPromise(
			registry
				.Authorize({ working_directory: root, workspace_id: "workspace-a" })
				.pipe(Effect.flip),
		);

		expect(denied).toMatchObject({
			_tag: "WorkspaceBoundedRegularFileStoreAuthorizationError",
			workspace_id: "workspace-a",
		});
	});

	it("rejects duplicate IDs and canonical roots before native acquisition", async () => {
		const root = await make_root();
		const other_root = await make_root();
		const alias = join(tmpdir(), `artisan native registry alias ${Date.now()}`);

		roots.push(alias);
		await fs.symlink(root, alias, "junction");

		for (const registrations of [
			[
				{ root, workspace_id: "workspace-a" },
				{ root: other_root, workspace_id: "workspace-a" },
			],
			[
				{ root, workspace_id: "workspace-a" },
				{ root: alias, workspace_id: "workspace-b" },
			],
		]) {
			const module = make_module();
			const failure = await Effect.runPromise(
				registry_effect(registrations, module).effect.pipe(Effect.flip),
			);

			expect(failure._tag).toBe("WorkspaceBoundedRegularFileStoreRegistrationError");
			expect(module.load_count).toBe(0);
			expect(module.constructed_roots).toEqual([]);
		}
	});

	it("rejects non-directory roots before native acquisition", async () => {
		const root = await make_root();
		const file = join(root, "file.txt");
		const module = make_module();

		await fs.writeFile(file, "file");

		const failure = await Effect.runPromise(
			registry_effect([{ root: file, workspace_id: "workspace-a" }], module).effect.pipe(
				Effect.flip,
			),
		);

		expect(failure._tag).toBe("WorkspaceBoundedRegularFileStoreRegistrationError");
		expect(module.load_count).toBe(0);
	});

	it("keeps lookup opaque and closes acquired stores after partial acquisition failure", async () => {
		const first_root = await make_root();
		const second_root = await make_root();
		const module = make_module(second_root);
		const failure = await Effect.runPromise(
			registry_effect(
				[
					{ root: first_root, workspace_id: "workspace-a" },
					{ root: second_root, workspace_id: "workspace-b" },
				],
				module,
			).effect.pipe(Effect.flip),
		);
		const empty_registry = await Effect.runPromise(registry_effect([]).effect);
		const not_found = await Effect.runPromise(empty_registry.Get("missing").pipe(Effect.flip));

		expect(failure._tag).toBe("NativeBoundedRegularFileStoreInitializationError");
		expect(module.close_count).toBe(1);
		expect(not_found).toMatchObject({
			_tag: "WorkspaceBoundedRegularFileStoreNotFoundError",
			workspace_id: "missing",
		});
		expect(JSON.stringify(not_found)).not.toContain(first_root);
	});
});
