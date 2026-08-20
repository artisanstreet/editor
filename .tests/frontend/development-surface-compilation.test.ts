import { createRequire } from "node:module";
import { globSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");

/** Svelte belongs to the frontend package, not the workspace root that runs the tests. */
const require_from_frontend = createRequire(resolve(workspace, "modules/frontend/package.json"));
const { parse } = (await import(
	pathToFileURL(require_from_frontend.resolve("svelte/compiler")).href
)) as { readonly parse: (source: string, options: { readonly modern: true }) => unknown };
const { transform_svelte_effect } = (await import(
	pathToFileURL(require_from_frontend.resolve("svelte-effect-runtime/runtime/transform")).href
)) as {
	readonly transform_svelte_effect: (
		source: string,
		filename: string,
	) => { readonly code: string };
};

/**
 * Development-only surfaces are stubbed out of the production build, which means
 * the ordinary frontend build never compiles them and cannot report their
 * syntax errors. Parsing them here restores the check the stub removes: without
 * it, a malformed development page is discovered only by opening it.
 */
const development_only_surfaces = [
	"modules/frontend/src/routes/debug/components/+page.svelte",
	"modules/frontend/src/routes/debug/emulator/+page.svelte",
	"modules/frontend/src/routes/debug/overlay/+page.svelte",
	"modules/frontend/src/routes/drafts/starting/+page.svelte",
];

/**
 * The starting-surface route is stubbed as one production module, but its
 * gallery imports its variants only in development. Parse those transitive
 * components too, so an invalid draft is found before somebody opens it.
 */
const development_svelte_sources = [
	...new Set([
		...development_only_surfaces,
		...globSync("modules/frontend/src/routes/drafts/starting/**/*.svelte", { cwd: workspace }),
	]),
].sort();

describe("development-only surface compilation", () => {
	for (const path of development_svelte_sources) {
		it(`parses ${path}`, () => {
			const source = readFileSync(resolve(workspace, path), "utf8");
			const transformed = transform_svelte_effect(source, path);

			expect(() => parse(transformed.code, { modern: true })).not.toThrow();
		});
	}

	/** The list is only useful while it names every stubbed surface. */
	it("covers every surface the build stubs", () => {
		const config = readFileSync(resolve(workspace, "modules/frontend/vite.config.ts"), "utf8");
		const stubbed = [...config.matchAll(/suffix: "([^"]+.svelte)"/gu)].map((match) => match[1]);

		expect(stubbed.length).toBeGreaterThan(0);
		for (const suffix of stubbed) {
			if (suffix === undefined) continue;
			expect(
				development_only_surfaces.some((path) => path.endsWith(suffix.replace(/^\//u, ""))),
				`${suffix} is stubbed but never parsed`,
			).toBe(true);
		}
	});
});
