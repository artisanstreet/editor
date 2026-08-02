import { cpSync, existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

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

const StageForgeRuntime = (forge_root: string, watching: boolean) => ({
	closeBundle: () => {
		const native_runtime_root = resolve(forge_root, "native-runtime");
		const frontend_source = resolve(import.meta.dirname, ".dist", "frontend");
		const migrations_source = resolve(import.meta.dirname, "modules/backend/drizzle");
		const stage_frontend = !watching;
		if (stage_frontend && !existsSync(frontend_source)) {
			throw new Error("Build the static frontend before Artisan Forge");
		}
		const node_pty_source = resolve(
			import.meta.dirname,
			"modules/backend/node_modules/node-pty",
		);
		const koffi_source = realpathSync(
			resolve(import.meta.dirname, "modules/engines/node_modules/koffi"),
		);
		const koffi_native_source = resolve(koffi_source, "..", "@koromix", "koffi-win32-x64");

		if (
			!existsSync(node_pty_source) ||
			!existsSync(koffi_source) ||
			!existsSync(koffi_native_source)
		) {
			throw new Error("node-pty and Koffi are required to package Artisan Forge");
		}

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
		stage(
			resolve(import.meta.dirname, ".scripts", "package", "update-user-path.ps1"),
			resolve(forge_root, "update-user-path.ps1"),
		);
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

/**
 * Builds Forge as an ESM Node application. The watcher owns rebuilds; the
 * development supervisor owns the clean stop/start of the daemon.
 */
export const CreateForgeRolldownConfig = (options: ForgeRolldownOptions = {}) => {
	const mode = options.mode ?? "production";
	const watching = options.watch ?? false;
	const forge_root = resolve(
		import.meta.dirname,
		".dist",
		mode === "validation" ? "validation/forge" : "forge",
	);

	return {
		input: {
			ae: resolve(import.meta.dirname, "modules/cli/src/entry.ts"),
			host: resolve(import.meta.dirname, "modules/forge/src/entry.ts"),
			"windows-process-host": resolve(
				import.meta.dirname,
				"modules/engines/src/process/windows-process-host.ts",
			),
		},
		platform: "node" as const,
		plugins: [StageForgeRuntime(forge_root, watching)],
		resolve: {
			alias: {
				"@artisan/forge": resolve(import.meta.dirname, "modules/forge/src/index.ts"),
				koffi: resolve(import.meta.dirname, "modules/desktop/src/koffi-shim.ts"),
				"node-pty": resolve(import.meta.dirname, "modules/desktop/src/node-pty-shim.ts"),
			},
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

export default CreateForgeRolldownConfig();
