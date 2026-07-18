import { cpSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
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

		if (!existsSync(node_pty_source)) {
			throw new Error("node-pty is required to package the Artisan desktop utility");
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
			"node-pty": resolve(import.meta.dirname, "modules/desktop/src/node-pty-shim.ts"),
		},
	},
	build: {
		outDir: ".dist/desktop",
		rollupOptions: {
			external: ["electron"],
			input: {
				main: resolve(import.meta.dirname, "modules/desktop/src/main.ts"),
				preload: resolve(import.meta.dirname, "modules/desktop/src/preload.ts"),
				utility: resolve(import.meta.dirname, "modules/desktop/src/utility.ts"),
			},
			output: { entryFileNames: "[name].js", format: "es" },
		},
		ssr: true,
		target: "node22",
	},
});
