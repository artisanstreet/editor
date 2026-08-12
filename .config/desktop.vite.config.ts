import { cpSync, existsSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { defineConfig } from "vite";

const workspace_root = resolve(import.meta.dirname, "..");
const desktop_root = resolve(workspace_root, ".dist", "desktop");
const workspace = JSON.parse(readFileSync(resolve(workspace_root, "package.json"), "utf8")) as {
	readonly version: string;
};
const desktop_version = process.env.ARTISAN_RELEASE_VERSION ?? workspace.version;

/**
 * The static frontend ships with a same-origin-only CSP for the Forge-served
 * browser copy. The editor's copy is loaded from `artisan://app` and talks to
 * the profile's loopback Forge cross-origin, so exactly that allowance is
 * patched in at staging time instead of weakening the browser bundle.
 */
const browser_connect_src = "connect-src 'self'";
/**
 * Chromium's CSP parser rejects bracketed IPv6 host sources, so only the IPv4
 * loopback is expressible here; profiles keep their default `127.0.0.1`
 * listener and an explicit `::1` profile cannot render in the editor.
 */
const editor_connect_src = "connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:*";

const patch_editor_csp = (frontend_root: string) => {
	const html_documents = readdirSync(frontend_root, {
		recursive: true,
		withFileTypes: true,
	}).filter((entry) => entry.isFile() && entry.name.endsWith(".html"));
	for (const entry of html_documents) {
		const path = join(entry.parentPath, entry.name);
		const document = readFileSync(path, "utf8");
		if (!document.includes(browser_connect_src)) {
			throw new Error(
				`The frontend CSP no longer declares \`${browser_connect_src}\`; update the editor staging patch in .config/desktop.vite.config.ts: ${path}`,
			);
		}
		writeFileSync(path, document.replaceAll(browser_connect_src, editor_connect_src));
	}
};

/**
 * Stages the renderer beside the launcher entry: the built static frontend is
 * copied into the payload (with the loopback CSP variant) so the packaged
 * editor renders it from `artisan://app` without any web hosting.
 */
const stage_desktop_payload = () => ({
	closeBundle: () => {
		const frontend_source = resolve(workspace_root, ".dist", "frontend");
		if (!existsSync(frontend_source)) {
			throw new Error("Build the static frontend before the Artisan editor payload");
		}
		const frontend_destination = resolve(desktop_root, "frontend");
		rmSync(frontend_destination, { force: true, recursive: true });
		cpSync(frontend_source, frontend_destination, {
			dereference: true,
			/**
			 * The editor's protocol handler serves plain files without content
			 * negotiation, so the precompressed siblings emitted for the
			 * Forge-hosted copy would only bloat the payload — and carry the
			 * unpatched same-origin CSP inside their compressed HTML.
			 */
			filter: (source) => !source.endsWith(".br") && !source.endsWith(".gz"),
			recursive: true,
		});
		patch_editor_csp(frontend_destination);
		writeFileSync(
			resolve(desktop_root, "package.json"),
			JSON.stringify({
				author: "Barekey",
				description: "Artisan Editor",
				main: "./main.js",
				name: "artisan-editor-desktop",
				packageManager: "npm@11.4.2",
				private: true,
				type: "module",
				version: desktop_version,
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
						version: desktop_version,
					},
				},
				requires: true,
				version: desktop_version,
			}),
		);
	},
	name: "stage-artisan-desktop-payload",
});

/** Bundles only privileged Electron entry points; the renderer stays a static asset tree. */
export default defineConfig({
	plugins: [stage_desktop_payload()],
	// Installed Electron applications cannot resolve workspace dependencies from
	// the repository. Bundle every JavaScript dependency; only Electron and Node
	// built-ins remain external through Rollup/Vite's platform handling.
	ssr: { noExternal: true },
	build: {
		outDir: ".dist/desktop",
		rollupOptions: {
			external: ["electron"],
			input: resolve(workspace_root, "modules/desktop/src/main.ts"),
			output: { entryFileNames: "[name].js", format: "es" },
		},
		ssr: true,
		target: "node22",
	},
});
