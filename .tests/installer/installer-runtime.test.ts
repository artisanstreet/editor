import { mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";
import { Effect } from "effect";

import { BootstrapInstaller } from "../../modules/installer/src/contract";
import {
	EnsureInstalledReleaseConfiguration,
	LoadBuiltInReleaseConfiguration,
	make_node_bootstrap_installer_layer,
} from "../../modules/installer/src/installer-runtime";

describe("bootstrap installer runtime", () => {
	it("fails without publication trust before mutating the installation root", async () => {
		const root = await mkdtemp(join(tmpdir(), "ae-installer-"));
		try {
			const layer = make_node_bootstrap_installer_layer({ ARTISAN_HOME: root });
			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						return yield* (yield* BootstrapInstaller).InstallFirstTime({
							argv: [],
							bootstrap_pid: 123,
							npm_executable: "C:\\Program Files\\nodejs\\npm.cmd",
							npm_prefix: "D:\\Portable\\npm-global",
							package_name: "artisan-editor",
						});
					}).pipe(Effect.provide(layer)),
				),
			).rejects.toMatchObject({
				_tag: "BootstrapInstallFailure",
				operation: "install",
			});
			expect(await readdir(root)).toEqual([]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("persists public trust for resume from a new shell without release environment", async () => {
		const root = await mkdtemp(join(tmpdir(), "ae-installer-"));
		const environment = {
			ARTISAN_HOME: root,
			ARTISAN_RELEASE_KEY_ID: "release-key",
			ARTISAN_RELEASE_OWNER: "sandersonstabo",
			ARTISAN_RELEASE_PUBLIC_KEY_BASE64: Buffer.from([1, 2, 3]).toString("base64"),
			ARTISAN_RELEASE_REPOSITORY: "artisan-editor",
		};
		try {
			const seeded = await Effect.runPromise(
				EnsureInstalledReleaseConfiguration(environment, root),
			);
			const resumed = await Effect.runPromise(
				EnsureInstalledReleaseConfiguration({ ARTISAN_HOME: root }, root),
			);
			expect(resumed).toEqual(seeded);
			const encoded = await readFile(join(root, "distribution.json"), "utf8");
			expect(encoded).not.toMatch(/private|secret/u);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("seeds official built-in release trust with an otherwise empty environment", async () => {
		const root = await mkdtemp(join(tmpdir(), "ae-installer-"));
		const public_key = Buffer.from([1, 2, 3]).toString("base64");
		try {
			const built_in = LoadBuiltInReleaseConfiguration({
				signing_key_id: "official-release-key",
				signing_public_key_base64: public_key,
			});
			await expect(
				Effect.runPromise(
					EnsureInstalledReleaseConfiguration({ ARTISAN_HOME: root }, root, built_in),
				),
			).resolves.toEqual({
				format_version: 1,
				owner: "sandersonstabo",
				repository: "artisan-editor",
				channel: "stable",
				signing_key_id: "official-release-key",
				signing_public_key_base64: public_key,
			});
			await expect(readFile(join(root, "distribution.json"), "utf8")).resolves.toContain(
				'"owner": "sandersonstabo"',
			);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("does not replace corrupt installed trust with environment input", async () => {
		const root = await mkdtemp(join(tmpdir(), "ae-installer-"));
		const path = join(root, "distribution.json");
		try {
			await writeFile(path, "{broken");
			await expect(
				Effect.runPromise(
					EnsureInstalledReleaseConfiguration(
						{
							ARTISAN_RELEASE_KEY_ID: "replacement",
							ARTISAN_RELEASE_OWNER: "replacement",
							ARTISAN_RELEASE_PUBLIC_KEY_BASE64: Buffer.from([4, 5, 6]).toString(
								"base64",
							),
							ARTISAN_RELEASE_REPOSITORY: "replacement",
						},
						root,
					),
				),
			).rejects.toBeDefined();
			expect(await readFile(path, "utf8")).toBe("{broken");
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});
});
