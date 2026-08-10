import { cpSync, existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { NtExecutable, NtExecutableResource, Resource } from "resedit";

const workspace_root = resolve(import.meta.dirname, "..");

export type ForgeBuildMode = "production" | "validation";

type ForgeRolldownOptions = {
	readonly mode?: ForgeBuildMode;
	readonly watch?: boolean;
};

/**
 * A staged file can be held open by a running development Forge on Windows.
 * Keep the previously staged version when that happens so a successful bundle
 * can still restart the daemon against a complete runtime.
 */
const stage = (from: string, to: string) => {
	try {
		cpSync(from, to, { dereference: true, recursive: true });
	} catch (error) {
		if (!existsSync(to)) throw error;
		console.warn(`[forge-build] kept the staged copy of ${to} (source copy blocked)`);
	}
};

/**
 * Task Manager lists a process by the FileDescription in its executable's
 * version resource, so the staged Node copy must carry Artisan branding or
 * the Forge shows up as "Node.js JavaScript Runtime". Rewriting the resource
 * invalidates Node's Authenticode signature; release signing, once it exists,
 * must re-sign this file afterwards.
 */
export const BrandForgeExecutable = (executable_path: string, product_version: string) => {
	const executable = NtExecutable.from(readFileSync(executable_path), { ignoreCert: true });
	const resources = NtExecutableResource.from(executable);
	const version_info = Resource.VersionInfo.fromEntries(resources.entries)[0];
	if (version_info === undefined) {
		throw new Error("the staged Node executable carries no version resource");
	}
	for (const language of version_info.getAllLanguagesForStringValues()) {
		version_info.setStringValues(language, {
			CompanyName: "Barekey",
			FileDescription: "Artisan Forge",
			InternalName: "Artisan Forge",
			OriginalFilename: "Artisan Forge.exe",
			ProductName: "Artisan",
		});
	}
	version_info.setFileVersion(product_version);
	version_info.setProductVersion(product_version);
	version_info.outputToResourceEntries(resources.entries);
	resources.outputResource(executable);
	writeFileSync(executable_path, Buffer.from(executable.generate()));
};

const StageForgeRuntime = (
	forge_root: string,
	options: { readonly stage_frontend: boolean; readonly watching: boolean },
) => ({
	closeBundle: () => {
		const { stage_frontend, watching } = options;
		const native_runtime_root = resolve(forge_root, "native-runtime");
		const frontend_source = resolve(workspace_root, ".dist", "frontend");
		const migrations_source = resolve(workspace_root, "modules/backend/drizzle");
		if (stage_frontend && !existsSync(frontend_source)) {
			throw new Error("Build the static frontend before Artisan Forge");
		}
		const node_pty_source = resolve(workspace_root, "modules/backend/node_modules/node-pty");
		const koffi_source = realpathSync(
			resolve(workspace_root, "modules/engines/node_modules/koffi"),
		);
		const koffi_native_source = resolve(koffi_source, "..", "@koromix", "koffi-win32-x64");
		if (
			!existsSync(node_pty_source) ||
			!existsSync(koffi_source) ||
			!existsSync(koffi_native_source)
		)
			throw new Error("node-pty and Koffi are required to package Artisan Forge");

		mkdirSync(forge_root, { recursive: true });
		const node_pty_destination = resolve(native_runtime_root, "node-pty");
		mkdirSync(node_pty_destination, { recursive: true });
		for (const path of ["LICENSE", "package.json", "lib", "prebuilds/win32-x64"]) {
			stage(resolve(node_pty_source, path), resolve(node_pty_destination, path));
		}

		const koffi_destination = resolve(native_runtime_root, "koffi");
		mkdirSync(resolve(koffi_destination, "src", "koffi"), { recursive: true });
		for (const path of [
			"index.cjs",
			"package.json",
			"src/koffi/index.cjs",
			"src/koffi/src/static.cjs",
		]) {
			stage(resolve(koffi_source, path), resolve(koffi_destination, path));
		}

		const koffi_native_destination = resolve(
			native_runtime_root,
			"@koromix",
			"koffi-win32-x64",
		);
		mkdirSync(koffi_native_destination, { recursive: true });
		for (const path of ["index.js", "package.json", "win32_x64"]) {
			stage(resolve(koffi_native_source, path), resolve(koffi_native_destination, path));
		}

		if (stage_frontend) stage(frontend_source, resolve(forge_root, "frontend"));
		stage(migrations_source, resolve(forge_root, "migrations"));
		for (const executable of ["Artisan Forge.exe", "node.exe"]) {
			stage(process.execPath, resolve(forge_root, executable));
		}
		if (!watching && process.platform === "win32") {
			const workspace = JSON.parse(
				readFileSync(resolve(workspace_root, "package.json"), "utf8"),
			) as { version: string };
			try {
				BrandForgeExecutable(resolve(forge_root, "Artisan Forge.exe"), workspace.version);
			} catch (error) {
				console.warn(`[forge-build] kept the unbranded Artisan Forge.exe (${error})`);
			}
		}
		writeFileSync(
			resolve(forge_root, "package.json"),
			JSON.stringify({ private: true, type: "module" }),
		);
		writeFileSync(
			resolve(forge_root, "ae.cmd"),
			'@echo off\r\nset "ARTISAN_NATIVE_RUNTIME=%~dp0native-runtime"\r\nset "NODE_PATH=%~dp0native-runtime;%NODE_PATH%"\r\n"%~dp0node.exe" "%~dp0ae.js" %*\r\n',
		);
	},
	name: "stage-artisan-forge-runtime",
});

const ForgeAliases = {
	"@artisan/forge": resolve(workspace_root, "modules/forge/src/index.ts"),
	koffi: resolve(workspace_root, "modules/desktop/src/koffi-shim.ts"),
	"node-pty": resolve(workspace_root, "modules/desktop/src/node-pty-shim.ts"),
};

/**
 * Builds Forge as an ESM Node application. The watcher owns rebuilds; the
 * development supervisor owns the clean stop/start of the daemon.
 */
export const CreateForgeRolldownConfig = (options: ForgeRolldownOptions = {}) => {
	const mode = options.mode ?? "production";
	const watching = options.watch ?? false;
	const forge_root = resolve(
		workspace_root,
		".dist",
		mode === "validation" ? "validation/forge" : "forge",
	);

	return {
		input: {
			ae: resolve(workspace_root, "modules/cli/src/entry.ts"),
			host: resolve(workspace_root, "modules/forge/src/host-entry.ts"),
			"windows-process-host": resolve(
				workspace_root,
				"modules/engines/src/process/windows-process-host-entry.ts",
			),
		},
		platform: "node" as const,
		plugins: [
			StageForgeRuntime(forge_root, {
				stage_frontend:
					!watching &&
					(mode === "production" ||
						existsSync(resolve(workspace_root, ".dist", "frontend"))),
				watching,
			}),
		],
		resolve: {
			alias: ForgeAliases,
		},
		output: {
			cleanDir: !watching,
			dir: forge_root,
			entryFileNames: "[name].js",
			format: "es" as const,
		},
		tsconfig: true,
		transform: { target: "node22" },
	};
};

/** Builds the one CommonJS payload supported by Node 24's SEA embedder. */
export const CreateForgeSeaRolldownConfig = () => ({
	input: resolve(workspace_root, "modules/forge/src/executable-entry.ts"),
	platform: "node" as const,
	resolve: { alias: ForgeAliases },
	output: {
		cleanDir: true,
		codeSplitting: false,
		dir: resolve(workspace_root, ".dist", "forge-sea-build"),
		entryFileNames: "forge-main.cjs",
		format: "cjs" as const,
	},
	tsconfig: true,
	transform: {
		/**
		 * The SEA payload is CommonJS under Node, where `import.meta` has no
		 * environment object. Effect's default ConfigProvider merges that browser
		 * convenience with `process.env`; replacing only its `env` member keeps
		 * the Node environment intact and makes the CJS boundary explicit.
		 */
		define: { "import.meta.env": "{}" },
		target: "node24",
	},
});

export default CreateForgeRolldownConfig();
