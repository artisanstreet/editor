import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	InstalledReleaseConfigurationStore,
	make_installed_release_configuration_store_layer,
} from "@artisan/distribution";
import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

const configuration = {
	format_version: 1,
	owner: "sandersonstabo",
	repository: "artisan-editor",
	channel: "stable",
	signing_key_id: "artisan-release-2026",
	signing_public_key_base64: Buffer.from([1, 2, 3]).toString("base64"),
} as const;

const WithStore = async (
	run: (root: string, store: InstalledReleaseConfigurationStore["Service"]) => Promise<void>,
) => {
	const root = await mkdtemp(join(tmpdir(), "artisan-release-configuration-"));
	const platform = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
	const layer = make_installed_release_configuration_store_layer(root).pipe(
		Layer.provide(platform),
	);
	try {
		const store = await Effect.runPromise(
			InstalledReleaseConfigurationStore.pipe(Effect.provide(layer)),
		);
		await run(root, store);
	} finally {
		await rm(root, { force: true, recursive: true });
	}
};

describe("InstalledReleaseConfigurationStore", () => {
	it("is non-mutating while public release configuration is absent", async () => {
		await WithStore(async (root, store) => {
			await expect(Effect.runPromise(store.Inspect())).resolves.toEqual({
				_tag: "Absent",
			});
			expect(await readdir(root)).toEqual([]);
		});
	});

	it("atomically persists only public origin and trust data", async () => {
		await WithStore(async (root, store) => {
			await Effect.runPromise(store.WriteAtomic(configuration));
			await expect(Effect.runPromise(store.Inspect())).resolves.toEqual({
				_tag: "Available",
				configuration,
			});
			const encoded = await readFile(join(root, "distribution.json"), "utf8");
			expect(encoded).toContain(configuration.signing_public_key_base64);
			expect(encoded).not.toMatch(/private|secret|ARTISAN_RELEASE_SIGNING_KEY/u);
			expect(await readdir(root)).toEqual(["distribution.json"]);
		});
	});

	it("reports corrupt installed configuration without replacing it", async () => {
		await WithStore(async (root, store) => {
			const path = join(root, "distribution.json");
			await writeFile(path, '{"format_version":1,"owner":');
			await expect(Effect.runPromise(store.Inspect())).resolves.toMatchObject({
				_tag: "Malformed",
				configuration_path: path,
			});
			expect(await readFile(path, "utf8")).toBe('{"format_version":1,"owner":');
		});
	});
});
