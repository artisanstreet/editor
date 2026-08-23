import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { Config, Effect, FileSystem, Option } from "effect";

/** The packaged host and `ae` executable deliberately share one Forge directory. */
export interface ForgeArtifact {
	readonly broker_executable_path: string;
	readonly executable_path: string;
	readonly host_entry_path: string;
	readonly migrations_path: string;
	readonly native_runtime_path: string;
	readonly node_executable_path: string;
	readonly static_frontend_root: string;
	readonly windows_process_host_path: string;
}

const OptionalString = (name: string) => Config.string(name).pipe(Config.option);
const ConfigValueOr = (value: Option.Option<string>, fallback: string) =>
	Option.getOrElse(value, () => fallback);

/**
 * Resolves packaged assets through Effect Config and FileSystem. Paths remain
 * relative to the CLI bundle and never depend on the caller's CWD.
 */
export const ResolveForgeArtifact = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const bundle_directory = dirname(fileURLToPath(import.meta.url));
	const configured_root = yield* OptionalString("ARTISAN_FORGE_ROOT");
	const bundled_host_exists = yield* file_system.exists(resolve(bundle_directory, "host.js"));
	const runtime_directory = ConfigValueOr(
		configured_root,
		bundled_host_exists
			? bundle_directory
			: resolve(bundle_directory, "..", "..", "..", ".dist", "forge"),
	);
	const [
		broker_executable_path,
		executable_path,
		host_entry_path,
		migrations_path,
		native_runtime_path,
		node_executable_path,
		static_frontend_root,
		windows_process_host_path,
	] = yield* Effect.all([
		OptionalString("ARTISAN_BROKER_PATH"),
		OptionalString("ARTISAN_FORGE_EXECUTABLE"),
		OptionalString("ARTISAN_FORGE_HOST"),
		OptionalString("ARTISAN_FORGE_MIGRATIONS"),
		OptionalString("ARTISAN_FORGE_NATIVE_RUNTIME"),
		OptionalString("ARTISAN_FORGE_NODE_EXECUTABLE"),
		OptionalString("ARTISAN_FORGE_STATIC_FRONTEND"),
		OptionalString("ARTISAN_WINDOWS_PROCESS_HOST"),
	]);
	return {
		broker_executable_path: ConfigValueOr(
			broker_executable_path,
			resolve(
				runtime_directory,
				process.platform === "win32" ? "Artisan Broker.exe" : "artisan-broker",
			),
		),
		executable_path: ConfigValueOr(
			executable_path,
			resolve(runtime_directory, "Artisan Forge.exe"),
		),
		host_entry_path: ConfigValueOr(host_entry_path, resolve(runtime_directory, "host.js")),
		migrations_path: ConfigValueOr(migrations_path, resolve(runtime_directory, "migrations")),
		native_runtime_path: ConfigValueOr(
			native_runtime_path,
			resolve(runtime_directory, "native-runtime"),
		),
		node_executable_path: ConfigValueOr(
			node_executable_path,
			resolve(runtime_directory, "node.exe"),
		),
		static_frontend_root: ConfigValueOr(
			static_frontend_root,
			resolve(runtime_directory, "frontend"),
		),
		windows_process_host_path: ConfigValueOr(
			windows_process_host_path,
			resolve(runtime_directory, "windows-process-host.js"),
		),
	} satisfies ForgeArtifact;
});
