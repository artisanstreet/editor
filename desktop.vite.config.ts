import { resolve } from "node:path";

import { defineConfig } from "vite";

/** Bundles only privileged Electron entry points; the renderer remains the frontend build. */
export default defineConfig({
	build: {
		outDir: ".dist/desktop",
		rollupOptions: {
			external: ["electron", "node-pty"],
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
