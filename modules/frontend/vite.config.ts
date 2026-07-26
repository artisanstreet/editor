import { fileURLToPath } from "node:url";

import adapter from "@sveltejs/adapter-static";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { effect } from "svelte-effect-runtime";
import { href } from "svelte-auto-href";
import { ts } from "svelte-global-typescript";
import { compose, kit } from "svelte-plugin-composer";
import { sv } from "svelte-sv-extension";

const WorkspaceSource = (relative_path: string) =>
	fileURLToPath(new URL(relative_path, import.meta.url));

const ForgeDevelopmentOrigin = process.env.ARTISAN_FORGE_DEV_ORIGIN ?? "http://127.0.0.1:4848";

export default defineConfig({
	resolve: {
		/**
		 * The frontend intentionally consumes the renderer-safe workspace sources.
		 * Keep these aliases explicit so browser development, including config
		 * reloads, and the immutable production build do not depend on pnpm's
		 * workspace junction layout.
		 */
		alias: [
			{
				find: "@artisan/transport/websocket/client",
				replacement: WorkspaceSource("../transport/src/websocket/client.ts"),
			},
			{
				find: "@artisan/transport/client",
				replacement: WorkspaceSource("../transport/src/client.ts"),
			},
			{
				find: "@artisan/transport",
				replacement: WorkspaceSource("../transport/src/index.ts"),
			},
			{
				find: "@artisan/protocol",
				replacement: WorkspaceSource("../protocol/src/index.ts"),
			},
			{
				find: "@artisan/catalog",
				replacement: WorkspaceSource("../catalog/src/index.ts"),
			},
		],
	},
	server: {
		fs: {
			allow: [WorkspaceSource("../..")],
		},
		proxy: {
			"/api/pair": {
				target: ForgeDevelopmentOrigin,
			},
			"/api/ws": {
				target: ForgeDevelopmentOrigin,
				ws: true,
			},
		},
	},
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
						"@artisan/data": "../data",
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
