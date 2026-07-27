import { cpSync, existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";

const forge_root = resolve(import.meta.dirname, ".dist", "forge");

const stage_forge_runtime = () => ({
	closeBundle: () => {
		const native_runtime_root = resolve(forge_root, "native-runtime");
		const frontend_source = resolve(import.meta.dirname, ".dist/frontend");
		const migrations_source = resolve(import.meta.dirname, "modules/backend/drizzle");
		if (!existsSync(frontend_source)) {
			throw new Error("Build the static frontend before Artisan Forge");
		}
		const node_pty_source = resolve(
			import.meta.dirname,
			"modules/backend/node_modules/node-pty",
		);
		const koffi_source = resolve(
			realpathSync(resolve(import.meta.dirname, "modules/engines/node_modules/koffi")),
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
			cpSync(resolve(node_pty_source, path), resolve(node_pty_destination, path), {
				dereference: true,
				recursive: true,
			});
		}
		const koffi_destination = resolve(native_runtime_root, "koffi");
		mkdirSync(resolve(koffi_destination, "src", "koffi"), { recursive: true });
		for (const path of [
			"index.cjs",
			"package.json",
			"src/koffi/index.cjs",
			"src/koffi/src/static.cjs",
		]) {
			cpSync(resolve(koffi_source, path), resolve(koffi_destination, path), {
				dereference: true,
			});
		}
		const koffi_native_destination = resolve(
			native_runtime_root,
			"@koromix",
			"koffi-win32-x64",
		);
		mkdirSync(koffi_native_destination, { recursive: true });
		for (const path of ["index.js", "package.json", "win32_x64"]) {
			cpSync(resolve(koffi_native_source, path), resolve(koffi_native_destination, path), {
				dereference: true,
				recursive: true,
			});
		}
		cpSync(frontend_source, resolve(forge_root, "frontend"), {
			dereference: true,
			recursive: true,
		});
		cpSync(migrations_source, resolve(forge_root, "migrations"), {
			dereference: true,
			recursive: true,
		});
		cpSync(process.execPath, resolve(forge_root, "Artisan Forge.exe"));
		/**
		 * Engine subprocess brokers require ordinary Node semantics. Keep this
		 * runtime distinct from the branded daemon executable so headless and
		 * packaged launches use the same explicit process boundary.
		 */
		cpSync(process.execPath, resolve(forge_root, "node.exe"));
		cpSync(
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

export default defineConfig({
	plugins: [stage_forge_runtime()],
	resolve: {
		alias: {
			"@artisan/forge": resolve(import.meta.dirname, "modules/forge/src/index.ts"),
			koffi: resolve(import.meta.dirname, "modules/desktop/src/koffi-shim.ts"),
			"node-pty": resolve(import.meta.dirname, "modules/desktop/src/node-pty-shim.ts"),
		},
	},
	ssr: { noExternal: true },
	build: {
		outDir: ".dist/forge",
		rollupOptions: {
			input: {
				ae: resolve(import.meta.dirname, "modules/cli/src/entry.ts"),
				host: resolve(import.meta.dirname, "modules/forge/src/entry.ts"),
				"windows-process-host": resolve(
					import.meta.dirname,
					"modules/engines/src/process/windows-process-host.ts",
				),
			},
			output: { entryFileNames: "[name].js", format: "es" },
		},
		ssr: true,
		target: "node22",
	},
});
