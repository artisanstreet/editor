import { writeFileSync } from "node:fs";
import { resolve } from "node:path";

import { defineConfig } from "vite";

const desktop_root = resolve(import.meta.dirname, ".dist", "desktop");

/** Writes the minimal launcher manifest consumed by Electron and electron-builder. */
const stage_desktop_manifest = () => ({
	closeBundle: () => {
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
	name: "stage-artisan-desktop-manifest",
});

/** Bundles only privileged Electron entry points; the renderer remains the frontend build. */
export default defineConfig({
	plugins: [stage_desktop_manifest()],
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
