import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_source = resolve("modules/backend/src/workspace");
const changes_source = join(workspace_source, "changes");
const line_count = (path: string) => readFileSync(path, "utf8").split(/\r?\n/u).length;

const TypeScriptFiles = (directory: string): ReadonlyArray<string> =>
	readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
		const path = join(directory, entry.name);

		return entry.isDirectory()
			? TypeScriptFiles(path)
			: entry.name.endsWith(".ts")
				? [path]
				: [];
	});

describe("workspace source structure", () => {
	it("keeps contextual names and every workspace production file below 1,000 lines", () => {
		for (const source_path of TypeScriptFiles(workspace_source)) {
			const source_name = source_path.slice(workspace_source.length + 1);

			expect(source_name.split(/[\\/]/u).at(-1)?.startsWith("workspace-")).toBe(false);
			expect(line_count(source_path), source_name).toBeLessThan(1_000);
		}
	});

	it("keeps change modules and the repository composition narrow", () => {
		for (const source_path of TypeScriptFiles(changes_source)) {
			expect(line_count(source_path), source_path).toBeLessThan(800);
		}

		expect(line_count(join(changes_source, "repository.ts"))).toBeLessThan(500);
	});
});
