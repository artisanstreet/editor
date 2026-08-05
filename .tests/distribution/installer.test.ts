import { mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { describe, expect, it } from "vitest";
import { Effect, Layer, ManagedRuntime } from "effect";

import { Activation, make_activation_layer } from "../../modules/distribution/src/activation";
import type { ActivatedInstallationManifest } from "../../modules/distribution/src/installation-manifest";
import {
	InstallationStore,
	make_installation_store_layer,
} from "../../modules/distribution/src/installation-store";
import {
	ArtifactDownloader,
	ArtifactStagingFailure,
	ArtifactStager,
	ForgeUpdateLifecycle,
	ForgeUpdateLifecycleFailure,
	InstallationHealth,
	InstallationHealthFailure,
	InstallationIntegrations,
	InstallationRollbackFailure,
	Installer,
	make_installer_layer,
	ReleaseSource,
} from "../../modules/distribution/src/installer";
import type {
	ReleaseArtifact,
	ReleaseManifest,
} from "../../modules/distribution/src/release-manifest";
import { ReleaseVerification } from "../../modules/distribution/src/verification";

const artifact_bytes = new Uint8Array([1, 2, 3]);
const sha256 = "a".repeat(64);

const MakeRelease = (version: string): ReleaseManifest => ({
	format_version: 1,
	product_version: version,
	editor_forge_compatibility_version: version,
	channel: "stable",
	signing_identity: { algorithm: "ed25519", key_id: "release-key" },
	minimum_installer_version: "0.1.0",
	minimum_cli_version: "0.1.0",
	artifacts: [
		{
			artifact_id: `artisan-windows-x64-${version}`,
			platform: "windows",
			architecture: "x64",
			archive_format: "zip",
			file_name: `artisan-${version}.zip`,
			byte_size: artifact_bytes.byteLength,
			sha256,
			archive_entries: ["Artisan.exe", "Artisan Forge.exe", "ae.exe"],
		},
	],
});

const MakeInstalled = (root: string, version: string): ActivatedInstallationManifest => ({
	format_version: 1,
	install_root: root,
	platform: "windows",
	architecture: "x64",
	channel: "stable",
	activation_state: "active",
	finalization_state: "complete",
	active_version: version,
	permanent_ae_path: join(root, "bin", "ae.exe"),
	artifact: {
		artifact_id: `artisan-windows-x64-${version}`,
		sha256,
		signing_key_id: "release-key",
	},
	components: { editor: true, forge: true },
	integrations: {},
	transaction: { state: "idle" },
	installed_at: "2026-07-27T00:00:00.000Z",
	updated_at: "2026-07-27T00:00:00.000Z",
});

interface HarnessOptions {
	readonly release: ReleaseManifest;
	readonly health_failure?: boolean;
	readonly resume_failure?: boolean;
	readonly staging_failures?: number;
}

const WithInstaller = async (
	options: HarnessOptions,
	operation: (services: {
		readonly activation: Activation["Service"];
		readonly health_checks: () => number;
		readonly integration_applies: () => number;
		readonly installer: Installer["Service"];
		readonly lifecycle_events: ReadonlyArray<string>;
		readonly root: string;
		readonly store: InstallationStore["Service"];
	}) => Promise<void>,
) => {
	const root = await mkdtemp(join(tmpdir(), "artisan-installer-"));
	let remaining_staging_failures = options.staging_failures ?? 0;
	let health_checks = 0;
	let integration_applies = 0;
	const lifecycle_events: Array<string> = [];
	const platform = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
	const store_layer = make_installation_store_layer(root).pipe(Layer.provide(platform));
	const activation_layer = make_activation_layer(root).pipe(Layer.provide(platform));
	const adapters = Layer.mergeAll(
		Layer.succeed(
			ReleaseSource,
			ReleaseSource.of({
				Resolve: () =>
					Effect.succeed({
						manifest: new TextEncoder().encode(JSON.stringify(options.release)),
						signature: {},
					}),
			}),
		),
		Layer.succeed(
			ArtifactDownloader,
			ArtifactDownloader.of({ Download: () => Effect.succeed(artifact_bytes) }),
		),
		Layer.succeed(
			ReleaseVerification,
			ReleaseVerification.of({
				VerifyArtifact: (_artifact: ReleaseArtifact, _bytes: Uint8Array) => Effect.void,
				VerifyManifest: () => Effect.succeed(options.release),
			}),
		),
		Layer.succeed(
			ArtifactStager,
			ArtifactStager.of({
				Stage: ({ version_path }) =>
					remaining_staging_failures > 0
						? Effect.sync(() => {
								remaining_staging_failures -= 1;
							}).pipe(
								Effect.andThen(
									Effect.fail(
										new ArtifactStagingFailure({
											cause: new Error("interrupted extraction"),
											target_version: options.release.product_version,
										}),
									),
								),
							)
						: Effect.tryPromise({
								try: () => mkdir(version_path, { recursive: true }),
								catch: (cause) =>
									new ArtifactStagingFailure({
										cause,
										target_version: options.release.product_version,
									}),
							}).pipe(Effect.asVoid),
			}),
		),
		Layer.succeed(
			InstallationIntegrations,
			InstallationIntegrations.of({
				Apply: ({ previous }) =>
					Effect.sync(() => {
						integration_applies += 1;
						return previous;
					}),
			}),
		),
		Layer.succeed(
			InstallationHealth,
			InstallationHealth.of({
				Check: (version) =>
					Effect.sync(() => {
						health_checks += 1;
					}).pipe(
						Effect.andThen(
							options.health_failure === true
								? Effect.fail(
										new InstallationHealthFailure({
											cause: new Error("unhealthy"),
											version,
										}),
									)
								: Effect.void,
						),
					),
			}),
		),
		Layer.succeed(
			ForgeUpdateLifecycle,
			ForgeUpdateLifecycle.of({
				Quiesce: (version) =>
					Effect.sync(() => {
						lifecycle_events.push(`quiesce:${version}`);
						return { was_running: true };
					}),
				Restore: (version, snapshot) =>
					Effect.sync(() => {
						lifecycle_events.push(`restore:${version}:${String(snapshot.was_running)}`);
					}),
				ResumeAndVerify: (version, snapshot) =>
					Effect.sync(() => {
						lifecycle_events.push(`resume:${version}:${String(snapshot.was_running)}`);
					}).pipe(
						Effect.andThen(
							options.resume_failure === true
								? Effect.fail(
										new ForgeUpdateLifecycleFailure({
											operation: "resume",
											version: options.release.product_version,
										}),
									)
								: Effect.void,
						),
					),
				VerifyCurrent: (version) =>
					Effect.sync(() => {
						lifecycle_events.push(`current:${version}`);
					}),
			}),
		),
	);
	const dependencies = Layer.mergeAll(store_layer, activation_layer, adapters, platform);
	const installer_layer = make_installer_layer({
		install_root: root,
		platform: "windows",
		architecture: "x64",
		channel: "stable",
		bootstrap_version: "0.1.0",
		cli_version: "0.1.0",
		permanent_ae_path: join(root, "bin", "ae.exe"),
		components: { editor: true, forge: true },
	}).pipe(Layer.provide(dependencies));
	const runtime = ManagedRuntime.make(
		Layer.mergeAll(installer_layer, store_layer, activation_layer),
	);

	try {
		await operation({
			activation: await runtime.runPromise(Activation),
			health_checks: () => health_checks,
			integration_applies: () => integration_applies,
			installer: await runtime.runPromise(Installer),
			lifecycle_events,
			root,
			store: await runtime.runPromise(InstallationStore),
		});
	} finally {
		await runtime.dispose();
		await rm(root, { force: true, recursive: true });
	}
};

describe("Installer", () => {
	it("records and completes a first installation transaction", async () => {
		await WithInstaller({ release: MakeRelease("0.2.0") }, async ({ installer, store }) => {
			const outcome = await Effect.runPromise(installer.Converge());
			expect(outcome).toMatchObject({
				_tag: "Installed",
				manifest: {
					activation_state: "active",
					active_version: "0.2.0",
					transaction: { state: "idle" },
				},
			});
			expect(await Effect.runPromise(store.Inspect())).toMatchObject({
				_tag: "Partial",
				manifest: {
					active_version: "0.2.0",
					finalization_state: "pending",
				},
			});
		});
	});

	it("resumes an interrupted first installation from its persisted transaction", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0"), staging_failures: 1 },
			async ({ installer, store }) => {
				await expect(Effect.runPromise(installer.Converge())).rejects.toBeInstanceOf(
					ArtifactStagingFailure,
				);
				expect(await Effect.runPromise(store.Inspect())).toMatchObject({
					_tag: "Partial",
					manifest: { transaction: { state: "verified" } },
				});
				expect(await Effect.runPromise(installer.Converge())).toMatchObject({
					_tag: "Installed",
					manifest: { active_version: "0.2.0" },
				});
			},
		);
	});

	it("does not leave an unhealthy first installation active", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0"), health_failure: true },
			async ({ activation, installer, store }) => {
				await expect(Effect.runPromise(installer.Converge())).rejects.toBeInstanceOf(
					InstallationHealthFailure,
				);
				expect(await Effect.runPromise(activation.ReadActive())).toEqual({
					_tag: "Absent",
				});
				expect(await Effect.runPromise(store.Inspect())).toMatchObject({
					_tag: "Partial",
					manifest: { transaction: { state: "integrating" } },
				});
			},
		);
	});

	it("updates beside the active version and publishes only after health succeeds", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0") },
			async ({ activation, installer, root, store }) => {
				await mkdir(join(root, "versions", "0.1.0"), { recursive: true });
				await Effect.runPromise(activation.Activate("0.1.0"));
				await Effect.runPromise(store.WriteAtomic(MakeInstalled(root, "0.1.0")));

				expect(await Effect.runPromise(installer.Converge())).toMatchObject({
					_tag: "Updated",
					manifest: {
						active_version: "0.2.0",
						previous_version: "0.1.0",
					},
				});
			},
		);
	});

	it("retains only the active and rollback versions after a successful update", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.3.0") },
			async ({ activation, installer, root, store }) => {
				const versions = join(root, "versions");
				await Promise.all([
					mkdir(join(versions, "0.1.0"), { recursive: true }),
					mkdir(join(versions, "0.2.0"), { recursive: true }),
					mkdir(join(versions, "manual-backup"), { recursive: true }),
				]);
				await writeFile(join(versions, "0.0.9"), "not a version directory");
				await Effect.runPromise(activation.Activate("0.2.0"));
				await Effect.runPromise(store.WriteAtomic(MakeInstalled(root, "0.2.0")));

				await expect(Effect.runPromise(installer.Converge())).resolves.toMatchObject({
					_tag: "Updated",
					manifest: {
						active_version: "0.3.0",
						previous_version: "0.2.0",
					},
				});
				expect((await readdir(versions)).sort()).toEqual([
					"0.0.9",
					"0.2.0",
					"0.3.0",
					"manual-backup",
				]);
			},
		);
	});

	it("verifies the active pointer, version layout, integrations, and health before returning Current", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0") },
			async ({ activation, health_checks, integration_applies, installer, root, store }) => {
				await mkdir(join(root, "versions", "0.2.0"), { recursive: true });
				await Effect.runPromise(activation.Activate("0.2.0"));
				await Effect.runPromise(store.WriteAtomic(MakeInstalled(root, "0.2.0")));

				await expect(Effect.runPromise(installer.Converge())).resolves.toMatchObject({
					_tag: "Current",
					manifest: { active_version: "0.2.0" },
				});
				expect(integration_applies()).toBe(1);
				expect(health_checks()).toBe(1);
			},
		);
	});

	it("rejects a matching manifest whose active pointer is missing", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0") },
			async ({ installer, root, store }) => {
				await mkdir(join(root, "versions", "0.2.0"), { recursive: true });
				await Effect.runPromise(store.WriteAtomic(MakeInstalled(root, "0.2.0")));

				await expect(Effect.runPromise(installer.Converge())).rejects.toMatchObject({
					_tag: "CurrentInstallationConvergenceFailure",
					code: "active_pointer",
					version: "0.2.0",
				});
			},
		);
	});

	it("rolls back the active pointer and manifest when post-activation health fails", async () => {
		await WithInstaller(
			{ release: MakeRelease("0.2.0"), resume_failure: true },
			async ({ activation, installer, lifecycle_events, root, store }) => {
				await mkdir(join(root, "versions", "0.0.9"), { recursive: true });
				await mkdir(join(root, "versions", "0.1.0"), { recursive: true });
				await Effect.runPromise(activation.Activate("0.1.0"));
				await Effect.runPromise(store.WriteAtomic(MakeInstalled(root, "0.1.0")));

				await expect(Effect.runPromise(installer.Converge())).rejects.toBeInstanceOf(
					InstallationRollbackFailure,
				);
				expect(await Effect.runPromise(activation.ReadActive())).toMatchObject({
					_tag: "Active",
					pointer: { active_version: "0.1.0" },
				});
				expect(await Effect.runPromise(store.Inspect())).toMatchObject({
					_tag: "Healthy",
					manifest: { active_version: "0.1.0", transaction: { state: "idle" } },
				});
				expect(lifecycle_events).toEqual([
					"quiesce:0.1.0",
					"resume:0.2.0:true",
					"restore:0.1.0:true",
				]);
				expect(await readdir(join(root, "versions"))).toContain("0.0.9");
			},
		);
	});
});
