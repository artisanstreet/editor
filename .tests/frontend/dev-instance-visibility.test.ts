import { existsSync, readdirSync, readFileSync } from "node:fs";
import { extname, join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	dev_title_marker,
	DevMarkedTitle,
	IsDevelopmentInstance,
} from "../../modules/frontend/src/lib/root/dev-instance";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

const text_extensions = new Set([".css", ".html", ".js", ".json", ".sv", ".svelte", ".ts"]);

const ReadTextTree = (
	root: string,
): ReadonlyArray<{ readonly path: string; readonly text: string }> =>
	readdirSync(root, { recursive: true, withFileTypes: true })
		.filter((entry) => entry.isFile() && text_extensions.has(extname(entry.name)))
		.map((entry) => {
			const path = join(entry.parentPath, entry.name);
			return { path, text: readFileSync(path, "utf8") };
		});

describe("development instance visibility", () => {
	it("marks a development instance only from an explicit health boolean", () => {
		expect(IsDevelopmentInstance({ development: true })).toBe(true);
		expect(IsDevelopmentInstance({ development: false })).toBe(false);
		expect(IsDevelopmentInstance({ development: "true" })).toBe(false);
		expect(IsDevelopmentInstance({})).toBe(false);
		expect(IsDevelopmentInstance(undefined)).toBe(false);
		expect(IsDevelopmentInstance("development")).toBe(false);
	});

	it("marks document titles idempotently", () => {
		expect(DevMarkedTitle("Artisan Editor")).toBe(`${dev_title_marker} Artisan Editor`);
		expect(DevMarkedTitle(`${dev_title_marker} Artisan Editor`)).toBe(
			`${dev_title_marker} Artisan Editor`,
		);
		expect(DevMarkedTitle("")).toBe(dev_title_marker);
	});

	it("wires the badge into the shell and the marker into route-owned titles", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const badge = Read("modules/frontend/src/routes/components/dev-instance-badge.sv");

		expect(layout).toContain("<DevInstanceBadge />");
		expect(badge).toContain('fetch(ForgeHttpUrl("/health")');
		expect(badge).toContain("IsDevelopmentInstance(");
		expect(badge).toContain("DevMarkedTitle(document.title)");
		expect(badge).toContain("new MutationObserver(");
		expect(badge).toContain("observer.disconnect()");
	});

	it("confines the development Forge origin to the dev-only Vite server block", () => {
		const vite_config = Read("modules/frontend/vite.config.ts");

		expect(vite_config).toContain(
			'process.env.ARTISAN_FORGE_DEV_ORIGIN ?? "http://127.0.0.1:4848"',
		);
		expect(vite_config).toContain("strictPort: true");
		expect(vite_config).toContain('host: "127.0.0.1"');
		expect(vite_config).toContain("port: FrontendDevelopmentPort");
		expect(vite_config).toContain('"/api": {');
		expect(vite_config).toContain("ws: true");
		expect(vite_config).toContain('"/health": {');
		/**
		 * The origin constant must feed only dev-server surfaces: the two proxy
		 * targets and the development pairing middleware, all of which exist
		 * exclusively inside `vite dev`. A `define` or an import of the constant
		 * would leak the development endpoint into the production bundle.
		 */
		expect(vite_config.match(/ForgeDevelopmentOrigin/g)).toHaveLength(4);
		expect(vite_config.split("target: ForgeDevelopmentOrigin")).toHaveLength(3);
		expect(vite_config).toContain("`${ForgeDevelopmentOrigin}/api/pair/request`");
		expect(vite_config).not.toContain("define:");
	});

	it("keeps frontend sources free of any hardcoded development Forge endpoint", () => {
		for (const source of ReadTextTree("modules/frontend/src")) {
			expect(source.text, source.path).not.toContain("127.0.0.1:4848");
			expect(source.text, source.path).not.toContain("127.0.0.1:4849");
			expect(source.text, source.path).not.toContain("ARTISAN_FORGE_DEV_ORIGIN");
		}
	});

	it.runIf(existsSync(resolve(".dist/frontend")))(
		"keeps the production bundle free of the development origin override",
		() => {
			const bundle = ReadTextTree(".dist/frontend");
			expect(bundle.length).toBeGreaterThan(0);
			for (const asset of bundle) {
				expect(asset.text, asset.path).not.toContain("127.0.0.1:4848");
				expect(asset.text, asset.path).not.toContain("ARTISAN_FORGE_DEV_ORIGIN");
			}
		},
	);
});
