import { readFileSync, readdirSync } from "node:fs";
import { extname, join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const frontend_source = resolve("modules/frontend/src");
const source_extensions = new Set([".css", ".html", ".sv", ".svelte", ".ts"]);

function source_files(directory: string): ReadonlyArray<string> {
	return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const path = join(directory, entry.name);

		return entry.isDirectory()
			? source_files(path)
			: source_extensions.has(extname(entry.name))
				? [path]
				: [];
	});
}

const files = source_files(frontend_source);
const sources = files.map((file) => ({ file, source: readFileSync(file, "utf8") }));
const aggregate_source = sources.map(({ source }) => source).join("\n");

function expect_source(pattern: RegExp, label: string) {
	const matches = sources.filter(({ source }) => pattern.test(source));

	expect(
		matches.map(({ file }) => relative(frontend_source, file)),
		`Expected frontend source to implement ${label}`,
	).not.toEqual([]);
}

describe("three-pane shell source layout", () => {
	it("defines the exact desktop grid and viewport ownership", () => {
		expect(aggregate_source).toContain("272px minmax(720px, 1fr) 340px");
		expect_source(/100dvh/, "a dynamic-viewport-height shell");
		expect_source(/editor-shell/, "the editor shell boundary");
	});

	it("collapses the right pane before the left pane becomes a rail", () => {
		expect(aggregate_source).toContain("max-width: 1367px");
		expect_source(
			/@media\s*\([^)]*max-width:\s*1279px[^)]*\)/,
			"the right-pane collapse breakpoint",
		);
		expect(aggregate_source).toContain("max-width: 999px");
		expect(aggregate_source).toMatch(/max-width:\s*1279px[\s\S]*?right-slot/);
		expect(aggregate_source).toMatch(/max-width:\s*999px[\s\S]*?left-rail-slot/);
	});

	it("provides mobile overlays for both secondary panes", () => {
		expect_source(
			/@media\s*\([^)]*max-width:\s*(?:999|799)px[^)]*\)/,
			"a mobile overlay breakpoint",
		);
		expect(aggregate_source).toContain("Open thread navigation");
		expect(aggregate_source).toContain("Open session pane");
		expect(aggregate_source).toMatch(/(?:t-panel|edge-panel)/);
		expect(aggregate_source).toContain("data-open");
	});

	it("exposes labelled regions and independently named pane content", () => {
		for (const label of ["Thread navigation", "Workspace", "Session"]) {
			expect(aggregate_source).toMatch(
				new RegExp(`<(?:aside|main|section)[^>]*aria-label=["']${label}["']`),
			);
		}
		expect_source(/thread-list/, "the independently scrollable thread list");
	});

	it("keeps workspace modes separate from file tabs", () => {
		expect(aggregate_source).toMatch(/workspace-mode-switcher/);
		expect(aggregate_source).toMatch(/file-tab-strip/);
		expect(aggregate_source.match(/role=["']group["']/g)?.length ?? 0).toBeGreaterThanOrEqual(
			2,
		);
	});

	it("labels preview-only data and supplies a reduced-motion state", () => {
		expect(aggregate_source).toContain("Preview data");
		expect_source(/prefers-reduced-motion:\s*reduce/, "the reduced-motion override");
	});

	it("contains no Barekey names or copied asset references", () => {
		const violations = sources
			.filter(({ file, source }) => /barekey|usebarekey/i.test(`${file}\n${source}`))
			.map(({ file }) => relative(frontend_source, file));

		expect(violations).toEqual([]);
	});
});
