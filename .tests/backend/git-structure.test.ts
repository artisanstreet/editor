import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const git_source = resolve("modules/backend/src/git");
const line_count = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

describe("Git source structure", () => {
	it("keeps contextual module names and every production file below 1,000 lines", () => {
		const source_names = readdirSync(git_source).filter((name) => name.endsWith(".ts"));

		expect(source_names.filter((name) => name.startsWith("git-"))).toEqual([]);
		for (const source_name of source_names) {
			expect(line_count(resolve(git_source, source_name)), source_name).toBeLessThan(1_000);
		}
	});

	it("keeps composition and read boundaries narrow", () => {
		expect(line_count(resolve(git_source, "repository.ts"))).toBeLessThan(500);
		expect(line_count(resolve(git_source, "read-service.ts"))).toBeLessThan(800);
	});
});
