import adapter from "@sveltejs/adapter-static";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { effect } from "svelte-effect-runtime";
import { href } from "svelte-auto-href";
import { ts } from "svelte-global-typescript";
import { compose, kit } from "svelte-plugin-composer";
import { sv } from "svelte-sv-extension";

export default defineConfig({
	plugins: [
		compose(
			[
				effect(),
				sv(),
				ts(),
				href(),
				kit({
					adapter: adapter({
						assets: "../../.dist/frontend",
						fallback: "index.html",
						pages: "../../.dist/frontend",
						precompress: true,
						strict: true,
					}),
					alias: {
						$: "./src/routes",
						$lib: "./src/lib",
					},
					compilerOptions: {
						experimental: {
							async: true,
						},
					},
				}),
			],
			{
				svelte_config: "direct",
			},
		),
		tailwindcss(),
	],
});
