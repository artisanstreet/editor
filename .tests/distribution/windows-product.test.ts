import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	IntegrationLifecycle,
	LoadWindowsProductConfigurationFromInstalled,
	LoadWindowsProductConfigurationFromEnvironment,
	MakeWindowsIntegrationSpecifications,
	ResolveWindowsInstallationRoot,
	WindowsProductIntegrations,
	make_windows_product_integrations_layer,
	type WindowsProductConfiguration,
} from "../../modules/distribution/src";

const Configuration = (
	root = "C:\\Users\\test\\AppData\\Local\\Artisan",
): WindowsProductConfiguration => ({
	installation_root: root,
	application_shortcut_path:
		"C:\\Users\\test\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Artisan Editor.lnk",
	architecture: "x64",
	channel: "stable",
	bootstrap_version: "0.1.0",
	cli_version: "0.1.0",
	release_owner: "sandersonstabo",
	release_repository: "artisan-editor",
	release_key_id: "release-key",
	release_public_key_der: new Uint8Array([1, 2, 3]),
});

describe("Windows product composition", () => {
	it("keeps protocol and shortcut targets stable across update and rollback", () => {
		const configuration = {
			...Configuration(),
			autostart_task_name: "Artisan Forge default",
		};
		const old_specifications = MakeWindowsIntegrationSpecifications(configuration, "0.1.0");
		const new_specifications = MakeWindowsIntegrationSpecifications(configuration, "0.2.0");

		expect(new_specifications).toEqual(old_specifications);
		expect(new_specifications.find(({ kind }) => kind === "protocol")).toMatchObject({
			content: '"C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\Artisan Editor.cmd" "%1"',
			path: "C:\\Users\\test\\AppData\\Local\\Artisan\\bin\\Artisan Editor.cmd",
		});
		expect(new_specifications.map(({ kind }) => kind)).toEqual([
			"ae_path",
			"protocol",
			"application_shortcut",
			"forge_start_shortcut",
			"forge_logs_shortcut",
			"uninstall_shortcut",
			"autostart",
		]);
		expect(
			new_specifications.find(({ kind }) => kind === "forge_start_shortcut"),
		).toMatchObject({
			path: expect.stringMatching(/Start Artisan Forge\.lnk$/u),
		});
		expect(new_specifications.find(({ kind }) => kind === "forge_logs_shortcut")).toMatchObject(
			{
				path: expect.stringMatching(/Artisan Forge Logs\.lnk$/u),
			},
		);
		expect(new_specifications.find(({ kind }) => kind === "uninstall_shortcut")).toMatchObject({
			path: expect.stringMatching(/Uninstall Artisan\.lnk$/u),
		});
		expect(new_specifications.find(({ kind }) => kind === "autostart")).toMatchObject({
			path: "Artisan Forge default",
		});
	});

	it("resolves the managed installation root without project state", () => {
		expect(
			ResolveWindowsInstallationRoot(
				{ LOCALAPPDATA: "C:\\Users\\test\\AppData\\Local" },
				"win32",
			),
		).toBe("C:\\Users\\test\\AppData\\Local\\Artisan");
	});

	it("fails missing trust configuration before creating installation state", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-product-config-"));
		try {
			await expect(
				Effect.runPromise(
					LoadWindowsProductConfigurationFromEnvironment(
						{ ARTISAN_HOME: root },
						"win32",
					).pipe(Effect.provide(NodeFileSystem.layer)),
				),
			).rejects.toMatchObject({ _tag: "WindowsProductConfigurationError" });
			expect(await readdir(root)).toEqual([]);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("preserves a foreign permanent launcher and fails before installing integrations", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-launcher-ownership-"));
		const stable_ae_path = join(root, "bin", "ae.cmd");
		let installed = false;
		try {
			await mkdir(join(root, "bin"), { recursive: true });
			await writeFile(stable_ae_path, "foreign", "utf8");
			const lifecycle = Layer.succeed(
				IntegrationLifecycle,
				IntegrationLifecycle.of({
					Inspect: () => Effect.succeed([]),
					Install: () =>
						Effect.sync(() => {
							installed = true;
							return {};
						}),
					Repair: () => Effect.succeed({}),
					Uninstall: () => Effect.succeed([]),
				}),
			);
			const layer = make_windows_product_integrations_layer(Configuration(root)).pipe(
				Layer.provide(Layer.mergeAll(NodeFileSystem.layer, NodePath.layer, lifecycle)),
			);

			await expect(
				Effect.runPromise(
					Effect.gen(function* () {
						yield* (yield* WindowsProductIntegrations).Apply("0.2.0", {});
					}).pipe(Effect.provide(layer)),
				),
			).rejects.toThrow();
			expect(installed).toBe(false);
			expect(await readFile(stable_ae_path, "utf8")).toBe("foreign");
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});

	it("decodes an explicit trusted public key without persisting it", async () => {
		const configuration = await Effect.runPromise(
			LoadWindowsProductConfigurationFromEnvironment(
				{
					APPDATA: "C:\\Users\\test\\AppData\\Roaming",
					ARTISAN_HOME: "C:\\Artisan",
					ARTISAN_RELEASE_KEY_ID: "release-key",
					ARTISAN_RELEASE_OWNER: "sandersonstabo",
					ARTISAN_RELEASE_PUBLIC_KEY_BASE64: Buffer.from([1, 2, 3]).toString("base64"),
					ARTISAN_RELEASE_REPOSITORY: "artisan-editor",
				},
				"win32",
			).pipe(Effect.provide(NodeFileSystem.layer)),
		);

		expect(configuration).toMatchObject({
			application_shortcut_path:
				"C:\\Users\\test\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Artisan\\Artisan Editor.lnk",
			installation_root: "C:\\Artisan",
			release_key_id: "release-key",
			release_owner: "sandersonstabo",
		});
		expect(configuration.release_public_key_der).toEqual(new Uint8Array([1, 2, 3]));
	});

	it("rehydrates a new-shell product configuration from installed public trust", async () => {
		const configuration = await Effect.runPromise(
			LoadWindowsProductConfigurationFromInstalled(
				{
					format_version: 1,
					owner: "sandersonstabo",
					repository: "artisan-editor",
					channel: "stable",
					signing_key_id: "release-key",
					signing_public_key_base64: Buffer.from([1, 2, 3]).toString("base64"),
				},
				{
					APPDATA: "C:\\Users\\test\\AppData\\Roaming",
					LOCALAPPDATA: "C:\\Users\\test\\AppData\\Local",
				},
				"win32",
			),
		);

		expect(configuration).toMatchObject({
			installation_root: "C:\\Users\\test\\AppData\\Local\\Artisan",
			release_key_id: "release-key",
			release_owner: "sandersonstabo",
			release_repository: "artisan-editor",
		});
	});
});
