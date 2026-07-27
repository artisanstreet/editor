import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import {
	InstallationStore,
	InstallationRootMismatch,
	make_installation_store_layer,
} from "../../modules/distribution/src/installation-store";
import type {
	ActivatedInstallationManifest,
	InstallationTransaction,
} from "../../modules/distribution/src/installation-manifest";

const MakeManifest = (
	root: string,
	transaction: InstallationTransaction = { state: "idle" },
): ActivatedInstallationManifest => ({
	format_version: 1,
	install_root: root,
	platform: "windows",
	architecture: "x64",
	channel: "stable",
	activation_state: "active",
	finalization_state: "complete",
	active_version: "0.1.0",
	permanent_ae_path: join(root, "bin", "ae.exe"),
	artifact: {
		artifact_id: "artisan-windows-x64",
		sha256: "a".repeat(64),
		signing_key_id: "release-key",
	},
	components: { editor: true, forge: true },
	integrations: {},
	transaction,
	installed_at: "2026-07-27T00:00:00.000Z",
	updated_at: "2026-07-27T00:00:00.000Z",
});

const WithStore = async (
	operation: (store: InstallationStore["Service"], root: string) => Promise<void>,
) => {
	const root = await mkdtemp(join(tmpdir(), "artisan-installation-store-"));
	const runtime = ManagedRuntime.make(
		make_installation_store_layer(root).pipe(
			Layer.provide(NodeFileSystem.layer),
			Layer.provide(NodePath.layer),
		),
	);

	try {
		await operation(await runtime.runPromise(InstallationStore), root);
	} finally {
		await runtime.dispose();
		await rm(root, { force: true, recursive: true });
	}
};

describe("InstallationStore", () => {
	it("distinguishes absent, partial, and healthy installations", async () => {
		await WithStore(async (store, root) => {
			expect(await Effect.runPromise(store.Inspect())).toEqual({ _tag: "Absent" });

			const active = MakeManifest(root);
			const {
				active_version: _,
				previous_version: __,
				artifact: ___,
				...unactivated_common
			} = active;
			await Effect.runPromise(
				store.WriteAtomic({
					...unactivated_common,
					activation_state: "unactivated",
					transaction: {
						state: "staged",
						target_version: "0.2.0",
						staging_path: join(root, "staging", "0.2.0"),
						started_at: "2026-07-27T00:01:00.000Z",
					},
				}),
			);
			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Partial",
				manifest: { transaction: { state: "staged" } },
			});

			await Effect.runPromise(store.WriteAtomic(MakeManifest(root)));
			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Healthy",
				manifest: { active_version: "0.1.0" },
			});
		});
	});

	it("reports malformed committed state without replacing or deleting it", async () => {
		await WithStore(async (store, root) => {
			const manifest_path = join(root, "installation.json");
			await writeFile(manifest_path, "{not-json", "utf8");

			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Malformed",
				manifest_path,
			});
			expect(await readFile(manifest_path, "utf8")).toBe("{not-json");
		});
	});

	it("migrates a legacy active manifest to pending instead of assuming setup completed", async () => {
		await WithStore(async (store, root) => {
			const { finalization_state: _, ...legacy } = MakeManifest(root);
			await writeFile(join(root, "installation.json"), `${JSON.stringify(legacy)}\n`, "utf8");

			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Partial",
				manifest: {
					activation_state: "active",
					finalization_state: "pending",
				},
			});
		});
	});

	it("ignores interrupted temporary writes and preserves the committed manifest", async () => {
		await WithStore(async (store, root) => {
			const committed = MakeManifest(root);
			await Effect.runPromise(store.WriteAtomic(committed));
			await writeFile(join(root, ".installation.json.tmp"), "{interrupted", "utf8");

			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Healthy",
				manifest: { active_version: committed.active_version },
			});
		});
	});

	it("validates before publication and confines manifests to the injected root", async () => {
		await WithStore(async (store, root) => {
			const committed = MakeManifest(root);
			await Effect.runPromise(store.WriteAtomic(committed));
			const original = await readFile(join(root, "installation.json"), "utf8");

			await expect(
				Effect.runPromise(
					store.WriteAtomic({
						...committed,
						install_root: join(root, "other"),
					}),
				),
			).rejects.toBeInstanceOf(InstallationRootMismatch);
			expect(await readFile(join(root, "installation.json"), "utf8")).toBe(original);
		});
	});
});
