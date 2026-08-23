import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import { brotliCompressSync, gzipSync } from "node:zlib";
import { Schema } from "effect";

import adapter from "@sveltejs/adapter-static";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig, type ViteDevServer } from "vite";
import { effect } from "svelte-effect-runtime";
import { href } from "svelte-auto-href";
import { ts } from "svelte-global-typescript";
import { compose, kit } from "svelte-plugin-composer";

const WorkspaceSource = (relative_path: string) =>
  fileURLToPath(new URL(relative_path, import.meta.url));

/**
 * Development-only Forge target. It lives exclusively inside `server`, which
 * `vite dev` consumes and `vite build` never emits, so the production bundle
 * keeps resolving its Forge strictly same-origin. The HMR page itself stays
 * same-origin too: pairing, health, and the control/stream WebSocket all pass
 * through this loopback proxy. The runner admits only that exact public editor
 * origin at the Forge boundary.
 */
const ForgeDevelopmentOrigin = process.env.ARTISAN_FORGE_DEV_ORIGIN ?? "http://127.0.0.1:4848";

/** Fixed so `ae open --origin` can deliver the pairing fragment deterministically. */
const FrontendDevelopmentPort = Number(process.env.ARTISAN_FRONTEND_DEV_PORT ?? "4849");
const FrontendPublicHostname = process.env.ARTISAN_FRONTEND_PUBLIC_HOSTNAME ?? "127.0.0.1";
const SentryConnectOrigin = (() => {
  const input =
    process.env.ARTISAN_SENTRY_EDITOR_ORIGIN ?? process.env.ARTISAN_SENTRY_EDITOR_DSN;
  if (input === undefined || input.trim() === "") return undefined;
  try {
    const url = new URL(input);
    return url.protocol === "https:" && url.username === "" && url.password === ""
      ? url.origin
      : undefined;
  } catch {
    return undefined;
  }
})();
const sentry_content_security_policy = () => ({
  closeBundle: {
    handler() {
      const output_root = resolve(import.meta.dirname, "../../.dist/frontend");
      if (!existsSync(output_root)) return;
      const documents = readdirSync(output_root, { recursive: true, withFileTypes: true }).filter(
        (entry) => entry.isFile() && entry.name.endsWith(".html"),
      );
      for (const entry of documents) {
        const path = join(entry.parentPath, entry.name);
        const patched = readFileSync(path, "utf8").replace(
          " __ARTISAN_SENTRY_CONNECT_ORIGIN__",
          SentryConnectOrigin === undefined ? "" : ` ${SentryConnectOrigin}`,
        );
        if (patched.includes("__ARTISAN_SENTRY_CONNECT_ORIGIN__")) {
          throw new Error(`Unable to finalize the Sentry CSP in ${path}`);
        }
        writeFileSync(path, patched);
        if (existsSync(`${path}.br`)) writeFileSync(`${path}.br`, brotliCompressSync(patched));
        if (existsSync(`${path}.gz`)) writeFileSync(`${path}.gz`, gzipSync(patched));
      }
    },
    order: "post" as const,
  },
  name: "artisan-sentry-content-security-policy",
  transformIndexHtml(html: string) {
    return html.replace(
      " __ARTISAN_SENTRY_CONNECT_ORIGIN__",
      SentryConnectOrigin === undefined ? "" : ` ${SentryConnectOrigin}`,
    );
  },
});
const DevelopmentSecrets = Schema.Struct({
  auth_token: Schema.optional(Schema.String),
});

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
    const secrets = Schema.decodeUnknownSync(Schema.fromJsonString(DevelopmentSecrets))(
      readFileSync(WorkspaceSource("../../.dist/dev/forge-home/secrets.json"), "utf8"),
    );
    return secrets.auth_token !== undefined && secrets.auth_token.length >= 32
      ? secrets.auth_token
      : undefined;
  } catch {
    return undefined;
  }
};

/**
 * Replaces development-only surfaces with a stub in a production build.
 *
 * A `dev` guard changes what renders, not what ships: Svelte compiles both
 * branches, so the page's markup and its scripted conversations would travel in
 * the bundle regardless. Stubbing the modules themselves is what makes the
 * exclusion real, and the emulator test asserts the result rather than trusting
 * the guard.
 */
const development_only_surfaces = () => {
  const stubbed = [
    {
      source: "export const emulator_scripts = [];",
      suffix: "/lib/conversation/emulator-scripts.ts",
    },
    {
      source: "<!-- development-only surface -->",
      suffix: "/routes/debug/emulator/+page.svelte",
    },
    {
      source: "<!-- development-only surface -->",
      suffix: "/routes/debug/overlay/+page.svelte",
    },
    {
      source: "<!-- development-only surface -->",
      suffix: "/routes/debug/components/+page.svelte",
    },
    /**
     * The draft gallery, and with it every variant, piece, and fixture it
     * imports: stubbing the page unroots the whole subtree.
     */
    {
      source: "<!-- development-only surface -->",
      suffix: "/routes/drafts/starting/+page.svelte",
    },
  ];

  return {
    enforce: "pre" as const,
    load(id: string) {
      const normalized = id.split("?")[0]?.replaceAll("\\", "/") ?? "";
      return stubbed.find((entry) => normalized.endsWith(entry.suffix))?.source;
    },
    name: "artisan-development-only-surfaces",
  };
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
  /** Private build artifact for Sentry upload; never staged into an installation. */
  build: { sourcemap: "hidden" },
  define: {
    __ARTISAN_RELEASE_COMMIT__: JSON.stringify(process.env.ARTISAN_RELEASE_COMMIT ?? "development"),
    __ARTISAN_RELEASE_VERSION__: JSON.stringify(
      process.env.ARTISAN_RELEASE_VERSION ?? "development",
    ),
    __ARTISAN_SENTRY_EDITOR_DSN__: JSON.stringify(process.env.ARTISAN_SENTRY_EDITOR_DSN ?? ""),
    __ARTISAN_TELEMETRY_ENVIRONMENT__: JSON.stringify(
      process.env.ARTISAN_TELEMETRY_ENVIRONMENT ?? "development",
    ),
  },
  /**
   * Every prebundled dependency the app reaches, declared up front. Left to
   * discovery, Vite finds the lazily-routed ones (CodeMirror behind the editor
   * route, the effect subpath entries, the Svelte libraries) only when a page
   * first imports them, re-optimizes mid-session, and force-reloads every open
   * tab — which a tab mid-navigation experiences as failed dynamic imports and
   * "Internal Error" preloads. Declared here, one boot-time pass covers them
   * and the dev session never reloads under the reader again.
   *
   * The icon package and shiki are excluded rather than enumerated: icons are
   * imported one file at a time and shiki one grammar file per fence language,
   * so each newly-used icon — or the first message fencing a language nobody
   * highlighted before — would otherwise be a fresh discovery and another
   * forced reload. Served as source (both ship plain ESM), a new one is just
   * another module.
   */
  optimizeDeps: {
    exclude: ["@tabler/icons-svelte", "shiki"],
    include: [
      "@codemirror/autocomplete",
      "@codemirror/commands",
      "@codemirror/lang-css",
      "@codemirror/lang-go",
      "@codemirror/lang-html",
      "@codemirror/lang-javascript",
      "@codemirror/lang-json",
      "@codemirror/lang-markdown",
      "@codemirror/lang-python",
      "@codemirror/lang-rust",
      "@codemirror/lang-sql",
      "@codemirror/lang-xml",
      "@codemirror/lang-yaml",
      "@codemirror/language",
      "@codemirror/lint",
      "@codemirror/search",
      "@codemirror/state",
      "@codemirror/view",
      "@comark/svelte",
      "@effect/platform-browser",
      "@lezer/highlight",
      "@selemondev/svgl-svelte",
      "beautiful-mermaid",
      "bits-ui",
      "comark",
      "comark/plugins/highlight",
      "comark/plugins/math",
      "comark/plugins/mermaid",
      "effect",
      "effect/unstable/http/HttpBody",
      "effect/unstable/http/HttpClient",
      "effect/unstable/persistence/KeyValueStore",
      "effect/unstable/rpc",
      "htmlparser2",
      "katex",
      "mode-watcher",
      "tailwind-merge",
      "tailwind-variants",
    ],
  },
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
    /** The runner supplies one exact Portless hostname; no wildcard is admitted. */
    allowedHosts: [FrontendPublicHostname],
    fs: {
      allow: [WorkspaceSource("../..")],
    },
    /**
     * Bound to explicit IPv4 loopback; Portless owns the browser-facing
     * HTTPS listener and forwards only the runner's exact hostname here.
     */
    host: "127.0.0.1",
    port: FrontendDevelopmentPort,
    proxy: {
      /**
       * The whole Forge API surface — pairing, instance listing, and the
       * `/api/ws` control/stream upgrade — forwards to the development
       * Forge. Rewriting `Host` to the private target keeps the proxy hop
       * loopback-only; the Forge separately admits the exact public editor
       * origin supplied by the runner.
       */
      "/api": {
        changeOrigin: true,
        target: ForgeDevelopmentOrigin,
        ws: true,
      },
      /** The dev-instance badge and connection gate probe Forge health. */
      "/health": {
        changeOrigin: true,
        target: ForgeDevelopmentOrigin,
      },
    },
    strictPort: true,
  },
  plugins: [
    sentry_content_security_policy(),
    ...(process.env.NODE_ENV === "production" ? [development_only_surfaces()] : []),
    development_pairing(),
    compose(
      [
        effect(),
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
    /**
     * Tailwind 4's build plugin is deliberately `enforce: "pre"`, but its
     * transform filter accepts CSS (including Svelte style virtual modules)
     * only. SER reports every pre-transform plugin and cannot express
     * that filter, so its ordering notice is a conservative false positive:
     * Tailwind cannot parse a `.svelte` module before `effect()` lowers it.
     *
     * Keep this separate from the composed Svelte-transform chain. Removing
     * Tailwind's priority would change its documented CSS pipeline merely to
     * silence a warning; the focused SER gate asserts this exception instead.
     */
    tailwindcss(),
  ],
});
