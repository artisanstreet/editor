import { cpSync, existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

import { defineConfig } from "vite";

const desktop_root = resolve(import.meta.dirname, ".dist", "desktop");

/** Stages runtime native packages for the separately built Artisan Forge. */
const stage_desktop_native_packages = () => ({
	closeBundle: () => {
		const native_runtime_root = resolve(desktop_root, "native-runtime");
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

		mkdirSync(native_runtime_root, { recursive: true });
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

		writeFileSync(
			resolve(desktop_root, "package.json"),
			JSON.stringify({
				main: "./main.js",
				name: "artisan-editor-desktop",
				packageManager: "npm@11.4.2",
				private: true,
				type: "module",
				version: "0.1.0",
			}),
		);
		writeFileSync(
			resolve(desktop_root, "package-lock.json"),
			JSON.stringify({
				lockfileVersion: 3,
				name: "artisan-editor-desktop",
				packages: {
					"": {
						name: "artisan-editor-desktop",
						version: "0.1.0",
					},
				},
				requires: true,
				version: "0.1.0",
			}),
		);
	},
	name: "stage-artisan-desktop-native-packages",
});

/** Bundles only privileged Electron entry points; the renderer remains the frontend build. */
export default defineConfig({
	plugins: [stage_desktop_native_packages()],
	resolve: {
		alias: {
			koffi: resolve(import.meta.dirname, "modules/desktop/src/koffi-shim.ts"),
			"node-pty": resolve(import.meta.dirname, "modules/desktop/src/node-pty-shim.ts"),
		},
	},
	// Installed Electron applications cannot resolve workspace dependencies from
	// the repository. Bundle every JavaScript dependency; only Electron and Node
	// built-ins remain external through Rollup/Vite's platform handling.
	ssr: { noExternal: true },
	build: {
		outDir: ".dist/desktop",
		rollupOptions: {
			external: ["electron"],
			input: resolve(import.meta.dirname, "modules/desktop/src/main.ts"),
			output: { entryFileNames: "[name].js", format: "es" },
		},
		ssr: true,
		target: "node22",
	},
});
