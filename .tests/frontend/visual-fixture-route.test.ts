import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const route_source = readFileSync(
	resolve("modules/frontend/src/routes/visual-fixtures/+page.sv"),
	"utf8",
);
const fixture_source = readFileSync(
	resolve("modules/frontend/src/routes/visual-fixtures/components/visual-fixture-page.sv"),
	"utf8",
);

describe("visual fixture route", () => {
	it("keeps the route compositional", () => {
		expect(route_source).toContain(
			'import VisualFixturePage from "./components/visual-fixture-page.sv"',
		);
		expect(route_source).toContain("<VisualFixturePage />");
		expect(route_source).not.toContain("<style>");
	});

	it("covers the complete initial component inventory", () => {
		for (const specimen of [
			"Typography",
			"Controls",
			"Modes and files",
			"Badges and status rows",
			"Loading and empty states",
			"Banners and permissions",
			"Inline diff",
			"Terminal chrome",
			"Fixture thread actions",
			'role="tooltip"',
		]) {
			expect(fixture_source, specimen).toContain(specimen);
		}
	});

	it("exposes explicit stress states for visual and accessibility review", () => {
		for (const state of [
			"Dark",
			"Light",
			"High contrast",
			"Reduced motion",
			"200% interface scale (simulated)",
			"Long labels",
		]) {
			expect(fixture_source, state).toContain(state);
		}

		expect(fixture_source).toContain("@media (prefers-reduced-motion: reduce)");
		expect(fixture_source).toContain('data-zoom="200"');
		expect(fixture_source).toContain("zoom: 2");
		expect(fixture_source).toContain('data-contrast="high"');
	});

	it("imports every rendered Tabler component under its actual markup name", () => {
		const tabler_import = fixture_source.match(
			/import\s*{([\s\S]*?)}\s*from\s*["']@tabler\/icons-svelte["']/,
		)?.[1];
		expect(tabler_import).toBeDefined();

		const imported_components = new Set(
			tabler_import!
				.split(",")
				.map(
					(specifier) =>
						specifier
							.trim()
							.split(/\s+as\s+/)
							.at(-1)!,
				)
				.filter(Boolean),
		);
		const fixture_markup = fixture_source.split("</script>", 2)[1]!.split("<style>", 1)[0]!;
		const rendered_components = new Set(
			Array.from(fixture_markup.matchAll(/<([A-Z][A-Za-z0-9]*)\b/g), (match) => match[1]!),
		);

		expect([...rendered_components].filter((name) => !imported_components.has(name))).toEqual(
			[],
		);
	});

	it("keeps interactions SER-owned and Effect.gen composed", () => {
		expect(fixture_source).toContain('<script lang="ts" effect>');
		expect(fixture_source).toContain("Effect.gen(function* ()");
		expect(fixture_source).toMatch(/on(?:click|keydown)=\{yield\*/);
		expect(fixture_source).not.toMatch(/Effect\.run[A-Z]/);
		expect(fixture_source).not.toMatch(/on(?:click|keydown)=\{\s*\([^)]*\)\s*=>/);
	});

	it("labels preview state and tells the truth about unavailable controls", () => {
		for (const label of [
			"Fixture",
			"Preview data",
			"Static fixture",
			"Local preview",
			"Backend unavailable",
			"unavailable",
		]) {
			expect(fixture_source, label).toContain(label);
		}

		expect(fixture_source.match(/disabled/g)?.length ?? 0).toBeGreaterThanOrEqual(4);
		expect(fixture_source).not.toMatch(/role=["']menu(?:item)?["']/);
	});

	it("contains no references to the visual research repository or its assets", () => {
		expect(fixture_source).not.toMatch(/(?:use)?barekey/i);
		expect(fixture_source).not.toMatch(/(?:logo|font)-barekey/i);
	});
});
