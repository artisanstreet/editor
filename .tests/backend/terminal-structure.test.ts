import { readdir, readFile } from "node:fs/promises";
import { basename, join } from "node:path";

import { describe, expect, it } from "vitest";

const terminal_root = join(process.cwd(), "modules", "backend", "src", "terminal");

describe("terminal source structure", () => {
	it("keeps contextual names and bounded modules", async () => {
		const source_files = (await readdir(terminal_root)).filter((file) => file.endsWith(".ts"));

		expect(source_files.filter((file) => basename(file).startsWith("terminal-"))).toEqual([]);

		for (const file of source_files) {
			const source = await readFile(join(terminal_root, file), "utf8");
			expect(source.split(/\r?\n/u).length, file).toBeLessThan(1_000);
			expect(source, file).not.toContain("JSON.parse");
		}

		const repository = await readFile(join(terminal_root, "repository.ts"), "utf8");
		expect(repository.split(/\r?\n/u).length).toBeLessThan(700);
	});
});
