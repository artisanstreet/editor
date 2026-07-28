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

/**
 * Development-only Forge target. It lives exclusively inside `server`, which
 * `vite dev` consumes and `vite build` never emits, so the production bundle
 * keeps resolving its Forge strictly same-origin. The HMR page itself stays
 * same-origin too: pairing, health, and the control/stream WebSocket all pass
 * through this loopback proxy, so no cross-origin allowance is needed on the
 * Forge side.
 */
const ForgeDevelopmentOrigin = process.env.ARTISAN_FORGE_DEV_ORIGIN ?? "http://127.0.0.1:4848";

/** Fixed so `ae open --origin` can deliver the pairing fragment deterministically. */
const FrontendDevelopmentPort = Number(process.env.ARTISAN_FRONTEND_DEV_PORT ?? "4849");

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
		/**
		 * Bound to the explicit IPv4 loopback so the origin the pairing
		 * capability lands on (`ae open --origin http://127.0.0.1:4849`) is
		 * exactly the origin the listener answers; the default `localhost` can
		 * resolve to `::1` only.
		 */
		host: "127.0.0.1",
		port: FrontendDevelopmentPort,
		proxy: {
			/**
			 * The whole Forge API surface — pairing, instance listing, and the
			 * `/api/ws` control/stream upgrade — forwards to the development
			 * Forge. `Host` is deliberately not rewritten: the Forge compares the
			 * browser's `Origin` against `Host`, so the untouched Vite loopback
			 * pair passes its same-origin policy without any server-side change.
			 */
			"/api": {
				target: ForgeDevelopmentOrigin,
				ws: true,
			},
			/** The dev-instance badge and connection gate probe Forge health. */
			"/health": {
				target: ForgeDevelopmentOrigin,
			},
		},
		strictPort: true,
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
