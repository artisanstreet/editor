import { posix, win32 } from "node:path";

import { Context, Data, Effect, FileSystem, Layer, Path, Schema } from "effect";

import { make_activation_layer } from "./activation";
import { AbsolutePath, Architecture, ReleaseChannel, SemanticVersion } from "./common";
import { type InstalledIntegrations, type InstallationManifest } from "./installation-manifest";
import { make_installation_store_layer } from "./installation-store";
import { IntegrationFailure, InstallationIntegrations, make_installer_layer } from "./installer";
import {
	IntegrationLifecycle,
	IntegrationLifecycleLive,
	type IntegrationSpecification,
} from "./integrations";
import {
	make_github_artifact_downloader_layer,
	make_github_release_source_layer,
	make_node_forge_update_lifecycle_layer,
	make_node_installation_health_layer,
	NodeDistributionPlatformLive,
	NodeHealthProcessHostLive,
	NodeSecureReleaseHttpLive,
	NodeZipArtifactStagerLive,
} from "./node-release-adapters";
import {
	NodeReleaseCryptographyLive,
	ReleaseVerificationLive,
	make_trusted_release_keys_layer,
} from "./verification";
import {
	InstalledReleaseConfiguration,
	type InstalledReleaseConfiguration as InstalledReleaseConfigurationValue,
} from "./release-configuration";
import {
	EncodeWindowsAutostart,
	EncodeWindowsShortcut,
	NodeWindowsNativeHostLive,
	WindowsIntegrationAdapterLive,
} from "./windows-native-adapter";
import { WindowsIntegrationPlatformLive } from "./windows-integrations";

export const WindowsProductConfiguration = Schema.Struct({
	installation_root: AbsolutePath,
	application_shortcut_path: AbsolutePath,
	autostart_task_name: Schema.optional(Schema.NonEmptyString),
	desktop_shortcut_path: Schema.optional(AbsolutePath),
	architecture: Architecture,
	channel: ReleaseChannel,
	bootstrap_version: SemanticVersion,
	cli_version: SemanticVersion,
	release_owner: Schema.NonEmptyString,
	release_repository: Schema.NonEmptyString,
	release_key_id: Schema.NonEmptyString,
	release_public_key_der: Schema.Uint8Array.check(Schema.isMinLength(1)),
});
export type WindowsProductConfiguration = typeof WindowsProductConfiguration.Type;

export class WindowsProductConfigurationError extends Data.TaggedError(
	"WindowsProductConfigurationError",
)<{ readonly cause: unknown }> {}

export const ResolveWindowsInstallationRoot = (
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) => {
	const path_service = platform === "win32" ? win32 : posix;
	const configured = environment.ARTISAN_HOME;
	if (configured !== undefined && path_service.isAbsolute(configured)) return configured;
	return platform === "win32"
		? win32.resolve(
				environment.LOCALAPPDATA ?? environment.USERPROFILE ?? process.cwd(),
				"Artisan",
			)
		: posix.resolve(
				environment.XDG_DATA_HOME ??
					posix.resolve(environment.HOME ?? process.cwd(), ".local", "share"),
				"artisan",
			);
};

const DecodePublicKey = (encoded: string) =>
	Effect.try({
		try: () => {
			const normalized = encoded.trim();
			const bytes = Buffer.from(normalized, "base64");
			if (bytes.byteLength === 0 || bytes.toString("base64") !== normalized)
				throw new Error("Release public key must be canonical base64 DER");
			return new Uint8Array(bytes);
		},
		catch: (cause) => new WindowsProductConfigurationError({ cause }),
	});

const MakeWindowsProductConfiguration = (
	release: InstalledReleaseConfigurationValue,
	environment: NodeJS.ProcessEnv,
	platform: NodeJS.Platform,
) =>
	Effect.gen(function* () {
		if (platform !== "win32")
			return yield* new WindowsProductConfigurationError({
				cause: new Error("The current production composition supports Windows only"),
			});
		const root = ResolveWindowsInstallationRoot(environment, platform);
		const app_data =
			environment.APPDATA ??
			win32.join(environment.USERPROFILE ?? root, "AppData", "Roaming");
		return yield* Schema.decodeUnknownEffect(WindowsProductConfiguration)({
			installation_root: root,
			application_shortcut_path: win32.join(
				app_data,
				"Microsoft",
				"Windows",
				"Start Menu",
				"Programs",
				"Artisan",
				"Artisan Editor.lnk",
			),
			architecture: process.arch === "arm64" ? "arm64" : "x64",
			channel: release.channel,
			bootstrap_version: "0.1.0",
			cli_version: "0.1.0",
			release_owner: release.owner,
			release_repository: release.repository,
			release_key_id: release.signing_key_id,
			release_public_key_der: yield* DecodePublicKey(release.signing_public_key_base64),
		}).pipe(Effect.mapError((cause) => new WindowsProductConfigurationError({ cause })));
	});

/** Decodes public release configuration supplied to a first-time bootstrap. */
export const LoadInstalledReleaseConfigurationFromEnvironment = (
	environment: NodeJS.ProcessEnv = process.env,
) =>
	Effect.gen(function* () {
		for (const name of [
			"ARTISAN_RELEASE_OWNER",
			"ARTISAN_RELEASE_REPOSITORY",
			"ARTISAN_RELEASE_KEY_ID",
		] as const)
			if (!environment[name])
				return yield* new WindowsProductConfigurationError({
					cause: new Error(`${name} is required`),
				});
		const file_system = yield* FileSystem.FileSystem;
		const encoded_key =
			environment.ARTISAN_RELEASE_PUBLIC_KEY_BASE64 ??
			(environment.ARTISAN_RELEASE_PUBLIC_KEY_FILE === undefined
				? undefined
				: yield* file_system
						.readFileString(environment.ARTISAN_RELEASE_PUBLIC_KEY_FILE)
						.pipe(
							Effect.mapError(
								(cause) => new WindowsProductConfigurationError({ cause }),
							),
						));
		if (encoded_key === undefined)
			return yield* new WindowsProductConfigurationError({
				cause: new Error(
					"ARTISAN_RELEASE_PUBLIC_KEY_BASE64 or ARTISAN_RELEASE_PUBLIC_KEY_FILE is required",
				),
			});
		const normalized_key = encoded_key.trim();
		yield* DecodePublicKey(normalized_key);
		return yield* Schema.decodeUnknownEffect(InstalledReleaseConfiguration)({
			format_version: 1,
			owner: environment.ARTISAN_RELEASE_OWNER,
			repository: environment.ARTISAN_RELEASE_REPOSITORY,
			channel: environment.ARTISAN_RELEASE_CHANNEL ?? "stable",
			signing_key_id: environment.ARTISAN_RELEASE_KEY_ID,
			signing_public_key_base64: normalized_key,
		}).pipe(Effect.mapError((cause) => new WindowsProductConfigurationError({ cause })));
	});

/** Rehydrates permanent ae configuration exclusively from installed public state. */
export const LoadWindowsProductConfigurationFromInstalled = (
	release: InstalledReleaseConfigurationValue,
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) => MakeWindowsProductConfiguration(release, environment, platform);

/** Loads explicit publication trust without creating installation state. */
export const LoadWindowsProductConfigurationFromEnvironment = (
	environment: NodeJS.ProcessEnv = process.env,
	platform: NodeJS.Platform = process.platform,
) =>
	Effect.gen(function* () {
		const release = yield* LoadInstalledReleaseConfigurationFromEnvironment(environment);
		return yield* MakeWindowsProductConfiguration(release, environment, platform);
	});

export class WindowsProductIntegrations extends Context.Service<
	WindowsProductIntegrations,
	{
		readonly Apply: (
			version: string,
			previous: InstalledIntegrations,
		) => Effect.Effect<InstalledIntegrations, unknown>;
		readonly Specifications: (
			manifest: InstallationManifest,
		) => Effect.Effect<ReadonlyArray<IntegrationSpecification>>;
	}
>()("Artisan/Distribution/WindowsProductIntegrations") {}

const StableAe = (installation_root: string) =>
	[
		"@echo off",
		"setlocal",
		`for /f "usebackq delims=" %%V in (\`powershell.exe -NoLogo -NoProfile -NonInteractive -Command "(Get-Content -Raw -LiteralPath '${installation_root.replaceAll("'", "''")}\\current' | ConvertFrom-Json).active_version"\`) do set "ARTISAN_VERSION=%%V"`,
		"if not defined ARTISAN_VERSION exit /b 1",
		`call "${installation_root}\\versions\\%ARTISAN_VERSION%\\bin\\ae.cmd" %*`,
		"",
	].join("\r\n");

const StableEditor = (installation_root: string) =>
	[
		"@echo off",
		"setlocal",
		`set "ARTISAN_AE_COMMAND=${installation_root}\\bin\\ae.cmd"`,
		`for /f "usebackq delims=" %%V in (\`powershell.exe -NoLogo -NoProfile -NonInteractive -Command "(Get-Content -Raw -LiteralPath '${installation_root.replaceAll("'", "''")}\\current' | ConvertFrom-Json).active_version"\`) do set "ARTISAN_VERSION=%%V"`,
		"if not defined ARTISAN_VERSION exit /b 1",
		`start "" "${installation_root}\\versions\\%ARTISAN_VERSION%\\editor\\Artisan Editor.exe" %*`,
		"",
	].join("\r\n");

export const MakeWindowsStableLaunchers = (installation_root: string) =>
	[
		{
			path: `${installation_root}\\bin\\ae.cmd`,
			content: StableAe(installation_root),
		},
		{
			path: `${installation_root}\\bin\\Artisan Editor.cmd`,
			content: StableEditor(installation_root),
		},
	] as const;

export const MakeWindowsIntegrationSpecifications = (
	configuration: WindowsProductConfiguration,
	_version: string,
): ReadonlyArray<IntegrationSpecification> => {
	const root = configuration.installation_root;
	const ae_path = `${root}\\bin\\ae.cmd`;
	const editor_path = `${root}\\bin\\Artisan Editor.cmd`;
	const shortcut_root = win32.dirname(configuration.application_shortcut_path);
	const Shortcut = (
		kind:
			| "application_shortcut"
			| "forge_logs_shortcut"
			| "forge_start_shortcut"
			| "uninstall_shortcut",
		name: string,
		description: string,
		target_path: string,
		arguments_: string,
	): IntegrationSpecification => ({
		kind,
		path: win32.join(shortcut_root, `${name}.lnk`),
		content: EncodeWindowsShortcut({
			arguments: arguments_,
			description,
			icon_path: editor_path,
			target_path,
			working_directory: `${root}\\bin`,
		}),
	});
	const specifications: Array<IntegrationSpecification> = [
		{
			kind: "ae_path",
			path: `${root}\\bin`,
			content: `PATH:${root}\\bin`,
		},
		{
			kind: "protocol",
			path: editor_path,
			content: `"${editor_path}" "%1"`,
		},
		Shortcut("application_shortcut", "Artisan Editor", "Artisan Editor", editor_path, ""),
		Shortcut(
			"forge_start_shortcut",
			"Start Artisan Forge",
			"Start Artisan Forge",
			ae_path,
			'start --profile "default"',
		),
		Shortcut(
			"forge_logs_shortcut",
			"Artisan Forge Logs",
			"Show Artisan Forge logs",
			ae_path,
			'logs --profile "default"',
		),
		Shortcut(
			"uninstall_shortcut",
			"Uninstall Artisan",
			"Uninstall Artisan",
			ae_path,
			"uninstall",
		),
	];
	if (configuration.desktop_shortcut_path !== undefined)
		specifications.push({
			kind: "desktop_shortcut",
			path: configuration.desktop_shortcut_path,
			content: EncodeWindowsShortcut({
				arguments: "",
				description: "Artisan Editor",
				icon_path: editor_path,
				target_path: editor_path,
				working_directory: `${root}\\bin`,
			}),
		});
	if (configuration.autostart_task_name !== undefined)
		specifications.push({
			kind: "autostart",
			path: configuration.autostart_task_name,
			content: EncodeWindowsAutostart({
				arguments: 'start --profile "default"',
				executable_path: ae_path,
				task_name: configuration.autostart_task_name,
				working_directory: `${root}\\bin`,
			}),
		});
	return specifications;
};

export const make_windows_product_integrations_layer = (
	configuration: WindowsProductConfiguration,
) =>
	Layer.effect(
		WindowsProductIntegrations,
		Effect.gen(function* () {
			const path_service = yield* Path.Path;
			const file_system = yield* FileSystem.FileSystem;
			const lifecycle = yield* IntegrationLifecycle;
			const [stable_ae, stable_editor] = MakeWindowsStableLaunchers(
				configuration.installation_root,
			);

			const EnsureStableFile = (path: string, content: string) =>
				Effect.gen(function* () {
					const exists = yield* file_system.exists(path);
					if (exists) {
						const current = yield* file_system.readFileString(path);
						if (current !== content)
							return yield* Effect.fail(
								new Error("Stable launcher path contains unowned content"),
							);
						return;
					}
					yield* file_system.makeDirectory(path_service.dirname(path), {
						recursive: true,
					});
					const temporary_path = `${path}.tmp`;
					yield* file_system.writeFileString(temporary_path, content, { flag: "wx" });
					yield* file_system
						.rename(temporary_path, path)
						.pipe(
							Effect.ensuring(
								file_system
									.remove(temporary_path, { force: true })
									.pipe(Effect.ignore),
							),
						);
				});

			const EnsureStableLaunchers = Effect.gen(function* () {
				yield* EnsureStableFile(stable_ae.path, stable_ae.content);
				yield* EnsureStableFile(stable_editor.path, stable_editor.content);
			});

			return WindowsProductIntegrations.of({
				Apply: (version, _previous) =>
					Effect.gen(function* () {
						yield* EnsureStableLaunchers;
						return yield* lifecycle.Install(
							MakeWindowsIntegrationSpecifications(configuration, version),
						);
					}),
				Specifications: (manifest) =>
					Effect.succeed(
						MakeWindowsIntegrationSpecifications(
							configuration,
							manifest.activation_state === "active"
								? manifest.active_version
								: manifest.transaction.target_version,
						),
					),
			});
		}),
	);

const make_installer_integrations_layer = (
	product_integrations: Layer.Layer<WindowsProductIntegrations>,
) =>
	Layer.effect(
		InstallationIntegrations,
		Effect.gen(function* () {
			const product = yield* WindowsProductIntegrations;
			return InstallationIntegrations.of({
				Apply: ({ previous, release }) =>
					product.Apply(release.product_version, previous).pipe(
						Effect.mapError(
							(cause) =>
								new IntegrationFailure({
									cause,
									target_version: release.product_version,
								}),
						),
					),
			});
		}).pipe(Effect.provide(product_integrations)),
	);

/** Full concrete Windows installer composition. Configuration is decoded before mutation. */
export const make_windows_product_layer = (input: unknown) =>
	Layer.unwrap(
		Schema.decodeUnknownEffect(WindowsProductConfiguration)(input).pipe(
			Effect.mapError((cause) => new WindowsProductConfigurationError({ cause })),
			Effect.map((configuration) => {
				const platform = NodeDistributionPlatformLive;
				const store = make_installation_store_layer(configuration.installation_root).pipe(
					Layer.provide(platform),
				);
				const activation = make_activation_layer(configuration.installation_root).pipe(
					Layer.provide(platform),
				);
				const native_integration = WindowsIntegrationAdapterLive.pipe(
					Layer.provide(NodeWindowsNativeHostLive),
				);
				const integration_platform = WindowsIntegrationPlatformLive.pipe(
					Layer.provide(native_integration),
				);
				const lifecycle = IntegrationLifecycleLive.pipe(
					Layer.provide(integration_platform),
				);
				const product_integrations = make_windows_product_integrations_layer(
					configuration,
				).pipe(Layer.provide(Layer.mergeAll(platform, lifecycle)));
				const installer_integrations =
					make_installer_integrations_layer(product_integrations);
				const http = NodeSecureReleaseHttpLive;
				const source = make_github_release_source_layer({
					owner: configuration.release_owner,
					repository: configuration.release_repository,
				}).pipe(Layer.provide(http));
				const downloader = make_github_artifact_downloader_layer(
					{
						owner: configuration.release_owner,
						repository: configuration.release_repository,
					},
					configuration.channel,
				).pipe(Layer.provide(http));
				const verification = ReleaseVerificationLive.pipe(
					Layer.provide(
						Layer.mergeAll(
							NodeReleaseCryptographyLive,
							make_trusted_release_keys_layer({
								[configuration.release_key_id]:
									configuration.release_public_key_der,
							}),
						),
					),
				);
				const stager = NodeZipArtifactStagerLive.pipe(Layer.provide(platform));
				const health = make_node_installation_health_layer(
					configuration.installation_root,
				).pipe(Layer.provide(Layer.mergeAll(NodeHealthProcessHostLive, platform)));
				const forge_update_lifecycle = make_node_forge_update_lifecycle_layer(
					configuration.installation_root,
				).pipe(Layer.provide(Layer.mergeAll(NodeHealthProcessHostLive, platform)));
				const dependencies = Layer.mergeAll(
					store,
					activation,
					source,
					downloader,
					verification,
					stager,
					health,
					forge_update_lifecycle,
					installer_integrations,
					lifecycle,
					product_integrations,
					platform,
				);
				const installer = make_installer_layer({
					install_root: configuration.installation_root,
					permanent_ae_path: `${configuration.installation_root}\\bin\\ae.cmd`,
					platform: "windows",
					architecture: configuration.architecture,
					channel: configuration.channel,
					bootstrap_version: configuration.bootstrap_version,
					cli_version: configuration.cli_version,
					components: { editor: true, forge: true },
				}).pipe(Layer.provide(dependencies));
				return Layer.mergeAll(
					installer,
					store,
					activation,
					lifecycle,
					product_integrations,
					platform,
				);
			}),
		),
	);
