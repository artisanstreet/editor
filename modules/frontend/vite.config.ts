import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import adapter from "@sveltejs/adapter-static";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type ViteDevServer } from "vite";
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

/**
 * The development pairing secret. The dev runner passes it explicitly; when
 * Vite is started by hand the same secret is read from the development home
 * the CLI and runner share. Missing both means self-pairing simply reports
 * unavailable and the manual `ae open` flow remains.
 */
const development_auth_token = (): string | undefined => {
	const explicit = process.env.ARTISAN_DEV_AUTH_TOKEN;
	if (explicit !== undefined && explicit.length >= 32) return explicit;
	try {
		const secrets = JSON.parse(
			readFileSync(WorkspaceSource("../../.dist/dev/forge-home/secrets.json"), "utf8"),
		) as { readonly auth_token?: unknown };
		return typeof secrets.auth_token === "string" && secrets.auth_token.length >= 32
			? secrets.auth_token
			: undefined;
	} catch {
		return undefined;
	}
};

/**
 * Development-only same-origin pairing: the dev server (a trusted local
 * process) mints a one-time code from the Forge with the bearer secret and
 * hands it to the page it itself served. This middleware exists only inside
 * `vite dev` — production bundles never carry it — and foreign pages can
 * never read the response because CORS blocks cross-origin reads while
 * Vite's host checks refuse rebound hostnames.
 */
const development_pairing = () => ({
	configureServer(server: ViteDevServer) {
		server.middlewares.use("/api/dev/pair-code", (request, response, next) => {
			if (request.method !== "POST") {
				next();
				return;
			}
			void (async () => {
				const token = development_auth_token();
				if (token === undefined) {
					response.writeHead(503, { "content-type": "application/json" });
					response.end(JSON.stringify({ error: "pairing_secret_unavailable" }));
					return;
				}
				const minted = await fetch(`${ForgeDevelopmentOrigin}/api/pair/request`, {
					headers: { authorization: `Bearer ${token}` },
					method: "POST",
				});
				if (!minted.ok) {
					response.writeHead(502, { "content-type": "application/json" });
					response.end(JSON.stringify({ error: "forge_unreachable" }));
					return;
				}
				const body = (await minted.json()) as { readonly code?: unknown };
				if (typeof body.code !== "string" || body.code.length === 0) {
					response.writeHead(502, { "content-type": "application/json" });
					response.end(JSON.stringify({ error: "invalid_pairing_code" }));
					return;
				}
				response.writeHead(200, {
					"cache-control": "no-store",
					"content-type": "application/json",
				});
				response.end(JSON.stringify({ code: body.code }));
			})().catch(() => {
				response.writeHead(500, { "content-type": "application/json" });
				response.end(JSON.stringify({ error: "pairing_failed" }));
			});
		});
	},
	name: "artisan-development-pairing",
});

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
		development_pairing(),
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
