import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { tmpdir } from "node:os";
import { win32 } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	Installer,
	InstalledReleaseConfigurationStore,
	IntegrationLifecycleLive,
	LoadWindowsProductConfigurationFromInstalled,
	MakeWindowsIntegrationSpecifications,
	MakeWindowsStableLaunchers,
	NodeWindowsNativeHostLive,
	ReleaseSourceFailure,
	WindowsIntegrationAdapterLive,
	WindowsIntegrationPlatformLive,
	make_installed_release_configuration_store_layer,
	make_installation_store_layer,
	make_windows_product_layer,
	ResolveWindowsInstallationRoot,
} from "@artisan/distribution";
import { Effect, FileSystem, Layer, Schema } from "effect";

import {
	DistributionIntegrationPlan,
	DistributionOperationsError,
	DistributionOperationsLive,
	DistributionRemoval,
} from "./distribution";
import { ForgeInstanceConfig } from "./instance";

export const ResolveInstallationRoot = ResolveWindowsInstallationRoot;

export interface WindowsDetachedUninstallPlan {
	readonly failure_path: string;
	readonly helper_path: string;
	readonly manual_cleanup_command: string;
	readonly readiness_path: string;
	readonly script: string;
}

/** Creates a helper outside the locked version tree and no broader deletion target. */
export const MakeWindowsDetachedUninstallPlan = (
	root: string,
	caller_pid: number,
	temporary_root = tmpdir(),
): WindowsDetachedUninstallPlan => {
	const escaped_root = root.replaceAll("'", "''");
	const manual_cleanup_command = `powershell.exe -NoProfile -NonInteractive -Command "Remove-Item -LiteralPath '${escaped_root}' -Recurse -Force"`;
	const helper_path = win32.join(temporary_root, `artisan-uninstall-${randomUUID()}.ps1`);
	const failure_path = `${helper_path}.failed.txt`;
	const readiness_path = `${helper_path}.ready`;
	return {
		failure_path,
		helper_path,
		manual_cleanup_command,
		readiness_path,
		script: [
			"$ErrorActionPreference = 'Stop'",
			`[System.IO.File]::WriteAllText('${readiness_path.replaceAll("'", "''")}', 'ready')`,
			`Wait-Process -Id ${caller_pid} -ErrorAction SilentlyContinue`,
			"$removed = $false",
			"for ($attempt = 0; $attempt -lt 20; $attempt++) {",
			"\ttry {",
			`\t\tRemove-Item -LiteralPath '${escaped_root}' -Recurse -Force -ErrorAction Stop`,
			"\t\t$removed = $true",
			"\t\tbreak",
			"\t} catch { Start-Sleep -Milliseconds 500 }",
			"}",
			"if (-not $removed) {",
			`\t[System.IO.File]::WriteAllText('${failure_path.replaceAll("'", "''")}', '${manual_cleanup_command.replaceAll("'", "''")}')`,
			"}",
			`Remove-Item -LiteralPath '${readiness_path.replaceAll("'", "''")}' -Force -ErrorAction SilentlyContinue`,
			"Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue",
			"",
		].join("\r\n"),
	};
};

const LoadInstalledWindowsProductConfiguration = (
	environment: NodeJS.ProcessEnv,
	host_platform: NodeJS.Platform,
	release_store: InstalledReleaseConfigurationStore["Service"],
) =>
	Effect.gen(function* () {
		const state = yield* release_store.Inspect();
		if (state._tag === "Absent")
			return yield* Effect.fail(new Error("Installed release configuration is absent"));
		if (state._tag === "Malformed") return yield* Effect.fail(state.cause);
		return yield* LoadWindowsProductConfigurationFromInstalled(
			state.configuration,
			environment,
			host_platform,
		);
	});

const InstallerFromInstalledConfiguration = (
	environment: NodeJS.ProcessEnv,
	host_platform: NodeJS.Platform,
	platform: Layer.Layer<FileSystem.FileSystem>,
	release_store_layer: Layer.Layer<InstalledReleaseConfigurationStore>,
) =>
	Layer.effect(
		Installer,
		Effect.gen(function* () {
			const release_store = yield* InstalledReleaseConfigurationStore;
			return Installer.of({
				Converge: () =>
					Effect.gen(function* () {
						const configuration = yield* LoadInstalledWindowsProductConfiguration(
							environment,
							host_platform,
							release_store,
						).pipe(Effect.provide(platform));
						return yield* Effect.gen(function* () {
							return yield* (yield* Installer).Converge();
						}).pipe(Effect.provide(make_windows_product_layer(configuration)));
					}).pipe(Effect.mapError((cause) => new ReleaseSourceFailure({ cause }))),
			});
		}),
	).pipe(Layer.provide(release_store_layer));

export const make_node_distribution_runtime_layer = (
	environment: NodeJS.ProcessEnv = process.env,
	host_platform: NodeJS.Platform = process.platform,
) => {
	const root = ResolveInstallationRoot(environment, host_platform);
	const platform = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
	const store = make_installation_store_layer(root).pipe(Layer.provide(platform));
	const release_store = make_installed_release_configuration_store_layer(root).pipe(
		Layer.provide(platform),
	);
	const lifecycle = IntegrationLifecycleLive.pipe(
		Layer.provide(
			WindowsIntegrationPlatformLive.pipe(
				Layer.provide(
					WindowsIntegrationAdapterLive.pipe(Layer.provide(NodeWindowsNativeHostLive)),
				),
			),
		),
	);
	const installer = InstallerFromInstalledConfiguration(
		environment,
		host_platform,
		platform,
		release_store,
	);
	const integration_plan = Layer.succeed(
		DistributionIntegrationPlan,
		DistributionIntegrationPlan.of({
			Specifications: (manifest) =>
				Effect.gen(function* () {
					const installed_release_store = yield* InstalledReleaseConfigurationStore;
					return yield* LoadInstalledWindowsProductConfiguration(
						environment,
						host_platform,
						installed_release_store,
					);
				}).pipe(
					Effect.provide(release_store),
					Effect.provide(platform),
					Effect.map((configuration) =>
						MakeWindowsIntegrationSpecifications(
							configuration,
							manifest.activation_state === "active"
								? manifest.active_version
								: manifest.transaction.target_version,
						),
					),
					Effect.mapError(
						(cause) =>
							new DistributionOperationsError({
								cause,
								code: "operation_unavailable",
								operation: "repair",
							}),
					),
				),
		}),
	);
	const removal = Layer.effect(
		DistributionRemoval,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const MapRemovalError = (cause: unknown) =>
				new DistributionOperationsError({
					cause,
					code: "operation_unavailable",
					operation: "uninstall",
				});
			return DistributionRemoval.of({
				RemoveForgeData: () =>
					Effect.gen(function* () {
						const config_path = win32.join(root, "config.json");
						const data_roots: Array<string> = [];
						if (yield* file_system.exists(config_path)) {
							const configuration = yield* file_system
								.readFileString(config_path)
								.pipe(
									Effect.flatMap((encoded) =>
										Effect.try({
											try: () => JSON.parse(encoded) as unknown,
											catch: (cause) => cause,
										}),
									),
									Effect.flatMap(Schema.decodeUnknownEffect(ForgeInstanceConfig)),
								);
							data_roots.push(configuration.data_root);
						}
						for (const data_root of data_roots) {
							if (win32.dirname(data_root) === data_root)
								return yield* Effect.fail(
									new Error("Refusing to remove a filesystem root"),
								);
						}
						/** Legacy `profiles/` trees predate the single-instance layout. */
						yield* Effect.forEach(
							[
								...new Set(data_roots),
								config_path,
								win32.join(root, "secrets.json"),
								win32.join(root, "state.json"),
								win32.join(root, "forge.log"),
								win32.join(root, "data"),
								win32.join(root, "profiles"),
							],
							(path) => file_system.remove(path, { force: true, recursive: true }),
							{ concurrency: 1, discard: true },
						);
					}).pipe(Effect.mapError(MapRemovalError)),
				RemoveProduct: () =>
					Effect.gen(function* () {
						for (const launcher of MakeWindowsStableLaunchers(root)) {
							const content = yield* file_system.readFileString(launcher.path);
							if (content !== launcher.content)
								return yield* Effect.fail(
									new Error("Refusing to remove an unowned stable launcher"),
								);
						}
						const plan = MakeWindowsDetachedUninstallPlan(root, process.pid);
						yield* file_system.writeFileString(plan.helper_path, plan.script, {
							flag: "wx",
						});
						yield* Effect.callback<void, Error>((resume) => {
							const escaped_helper_path = plan.helper_path.replaceAll('"', '""');
							const launcher = spawn(
								process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe",
								[
									"/d",
									"/s",
									"/c",
									`start "" /b powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "${escaped_helper_path}"`,
								],
								{
									shell: false,
									stdio: "ignore",
									windowsHide: true,
									windowsVerbatimArguments: true,
								},
							);
							let completed = false;
							launcher.once("error", (cause) => {
								if (completed) return;
								completed = true;
								resume(Effect.fail(cause));
							});
							launcher.once("close", (exit_code) => {
								if (completed) return;
								completed = true;
								resume(
									exit_code === 0
										? Effect.void
										: Effect.fail(
												new Error(
													`Detached uninstall launcher exited ${String(exit_code)}`,
												),
											),
								);
							});
							return Effect.sync(() => {
								if (!completed && launcher.exitCode === null) launcher.kill();
							});
						});
						let helper_ready = false;
						for (let attempt = 0; attempt < 100; attempt++) {
							helper_ready = yield* file_system.exists(plan.readiness_path);
							if (helper_ready) break;
							yield* Effect.sleep("50 millis");
						}
						if (!helper_ready)
							return yield* Effect.fail(
								new Error("Detached uninstall helper did not initialize"),
							);
						return { manual_cleanup_command: plan.manual_cleanup_command };
					}).pipe(Effect.mapError(MapRemovalError)),
			});
		}),
	).pipe(Layer.provide(platform));
	const dependencies = Layer.mergeAll(
		store,
		release_store,
		installer,
		lifecycle,
		integration_plan,
		removal,
		platform,
	);
	return Layer.mergeAll(
		DistributionOperationsLive.pipe(Layer.provide(dependencies)),
		store,
		release_store,
		lifecycle,
		platform,
	);
};
