import { resolve } from "node:path";

import { defineConfig } from "vite";

/** Electron preloads are CommonJS even though the privileged main runtime is ESM. */
export default defineConfig({
	build: {
		outDir: ".dist/desktop",
		rollupOptions: {
			external: ["electron"],
			input: { preload: resolve(import.meta.dirname, "modules/desktop/src/preload.ts") },
			output: { entryFileNames: "[name].cjs", format: "cjs" },
		},
		emptyOutDir: false,
		ssr: true,
		target: "node22",
	},
});
