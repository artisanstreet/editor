import { cpSync, existsSync, mkdirSync, realpathSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

import { defineConfig } from "vite";

const desktop_root = resolve(import.meta.dirname, ".dist", "desktop");

/** Stages only runtime native packages beside the utility entry for Node resolution. */
const stage_desktop_native_packages = () => ({
	closeBundle: () => {
		const native_runtime_root = resolve(desktop_root, "native-runtime");
		const node_pty_source = resolve(
			import.meta.dirname,
			"modules/backend/node_modules/node-pty",
		);
		const koffi_source = resolve(
			realpathSync(resolve(import.meta.dirname, "modules/engines/node_modules/koffi")),
			"..",
			"@koromix",
			"koffi-win32-x64",
		);

		if (!existsSync(node_pty_source) || !existsSync(koffi_source)) {
			throw new Error(
				"node-pty and Koffi are required to package the Artisan desktop utility",
			);
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

		const koffi_destination = resolve(native_runtime_root, "@koromix", "koffi-win32-x64");
		mkdirSync(koffi_destination, { recursive: true });
		for (const path of ["index.js", "package.json", "win32_x64"]) {
			cpSync(resolve(koffi_source, path), resolve(koffi_destination, path), {
				dereference: true,
				recursive: true,
			});
		}

		const bounded_source = resolve(import.meta.dirname, ".dist/bounded-file-store-native");

		if (!existsSync(resolve(bounded_source, "index.cjs"))) {
			throw new Error(
				"Missing .dist/bounded-file-store-native/index.cjs; build the production native addon before packaging desktop",
			);
		}

		const bounded_destination = resolve(
			native_runtime_root,
			"@artisan",
			"bounded-file-store-native",
		);

		mkdirSync(bounded_destination, { recursive: true });
		for (const path of [
			"bounded_file_store_native.win32-x64-msvc.node",
			"index.cjs",
			"index.d.ts",
		]) {
			cpSync(resolve(bounded_source, path), resolve(bounded_destination, path), {
				dereference: true,
			});
		}
		writeFileSync(
			resolve(bounded_destination, "package.json"),
			JSON.stringify({
				main: "./index.cjs",
				name: "@artisan/bounded-file-store-native",
				private: true,
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
			input: {
				main: resolve(import.meta.dirname, "modules/desktop/src/main.ts"),
				utility: resolve(import.meta.dirname, "modules/desktop/src/utility.ts"),
			},
			output: { entryFileNames: "[name].js", format: "es" },
		},
		ssr: true,
		target: "node22",
	},
});
