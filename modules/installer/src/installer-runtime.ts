import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	Installer,
	InstalledReleaseConfiguration,
	InstalledReleaseConfigurationStore,
	LoadInstalledReleaseConfigurationFromEnvironment,
	LoadWindowsProductConfigurationFromInstalled,
	ResolveWindowsInstallationRoot,
	make_installed_release_configuration_store_layer,
	make_windows_product_layer,
	type InstalledReleaseConfiguration as InstalledReleaseConfigurationValue,
} from "@artisan/distribution";
import { Effect, Layer, Schema } from "effect";

import { BootstrapInstallFailure, BootstrapInstaller, type BootstrapInvocation } from "./contract";

declare const __ARTISAN_RELEASE_SIGNING_KEY_ID__: string;
declare const __ARTISAN_RELEASE_PUBLIC_KEY_BASE64__: string;

/** Public trust compiled into the disposable official bootstrap. */
export const LoadBuiltInReleaseConfiguration = (
	input: {
		readonly signing_key_id: string;
		readonly signing_public_key_base64: string;
	} = {
		signing_key_id:
			typeof __ARTISAN_RELEASE_SIGNING_KEY_ID__ === "string"
				? __ARTISAN_RELEASE_SIGNING_KEY_ID__
				: "",
		signing_public_key_base64:
			typeof __ARTISAN_RELEASE_PUBLIC_KEY_BASE64__ === "string"
				? __ARTISAN_RELEASE_PUBLIC_KEY_BASE64__
				: "",
	},
) =>
	Schema.decodeUnknownEffect(InstalledReleaseConfiguration)({
		format_version: 1,
		owner: "sandersonstabo",
		repository: "artisan-editor",
		channel: "stable",
		signing_key_id: input.signing_key_id,
		signing_public_key_base64: input.signing_public_key_base64,
	});

/** Seeds public release trust once, then treats installed state as authoritative. */
export const EnsureInstalledReleaseConfiguration = (
	environment: NodeJS.ProcessEnv,
	installation_root = ResolveWindowsInstallationRoot(environment, "win32"),
	built_in: Effect.Effect<
		InstalledReleaseConfigurationValue,
		unknown,
		never
	> = LoadBuiltInReleaseConfiguration(),
) =>
	Effect.gen(function* () {
		const platform = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
		const release_store_layer = make_installed_release_configuration_store_layer(
			installation_root,
		).pipe(Layer.provide(platform));
		const release_store = yield* InstalledReleaseConfigurationStore.pipe(
			Effect.provide(release_store_layer),
		);
		const state = yield* release_store.Inspect();
		if (state._tag === "Available") return state.configuration;
		if (state._tag === "Malformed") return yield* Effect.fail(state.cause);
		const has_environment_override =
			environment.ARTISAN_RELEASE_OWNER !== undefined ||
			environment.ARTISAN_RELEASE_REPOSITORY !== undefined ||
			environment.ARTISAN_RELEASE_KEY_ID !== undefined ||
			environment.ARTISAN_RELEASE_PUBLIC_KEY_BASE64 !== undefined ||
			environment.ARTISAN_RELEASE_PUBLIC_KEY_FILE !== undefined;
		const seed = has_environment_override
			? yield* LoadInstalledReleaseConfigurationFromEnvironment(environment).pipe(
					Effect.provide(NodeFileSystem.layer),
				)
			: yield* built_in;
		yield* release_store.WriteAtomic(seed);
		return seed;
	});

const Converge = (
	operation: BootstrapInstallFailure["operation"],
	environment: NodeJS.ProcessEnv,
) =>
	Effect.gen(function* () {
		const installation_root = ResolveWindowsInstallationRoot(environment, "win32");
		const release = yield* EnsureInstalledReleaseConfiguration(environment, installation_root);
		const configuration = yield* LoadWindowsProductConfigurationFromInstalled(
			release,
			environment,
			"win32",
		);
		const outcome = yield* Effect.gen(function* () {
			return yield* (yield* Installer).Converge();
		}).pipe(Effect.provide(make_windows_product_layer(configuration)));
		return { permanent_ae_path: outcome.manifest.permanent_ae_path };
	}).pipe(
		Effect.mapError(
			(cause) =>
				new BootstrapInstallFailure({
					cause,
					operation,
				}),
		),
	);

/** First install and interrupted resume converge through the same persisted installer state. */
export const make_node_bootstrap_installer_layer = (environment: NodeJS.ProcessEnv = process.env) =>
	Layer.succeed(
		BootstrapInstaller,
		BootstrapInstaller.of({
			InstallFirstTime: (_invocation: BootstrapInvocation) =>
				Converge("install", environment),
			Resume: (_manifest, _invocation) => Converge("resume", environment),
		}),
	);
