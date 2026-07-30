import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const catalog_source = join(process.cwd(), "modules", "catalog", "src");

const SourceFiles = (directory: string): ReadonlyArray<string> =>
	readdirSync(directory).flatMap((entry) => {
		const path = join(directory, entry);

		return statSync(path).isDirectory()
			? SourceFiles(path)
			: path.endsWith(".ts")
				? [path]
				: [];
	});

const LineCount = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

describe("catalog source structure", () => {
	it("keeps contextual catalog modules below the structural ceiling", () => {
		const oversized = SourceFiles(catalog_source)
			.map((path) => ({ path, lines: LineCount(path) }))
			.filter(({ lines }) => lines >= 800);

		expect(oversized).toEqual([]);
	});

	it("keeps the manifest composition boundary narrow", () => {
		expect(LineCount(join(catalog_source, "model-manifest.ts"))).toBeLessThan(250);
	});
});
