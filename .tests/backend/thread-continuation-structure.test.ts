import { readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const source_directory = resolve(
	process.cwd(),
	"modules/backend/src/persistence/thread-continuation",
);

describe("thread continuation persistence structure", () => {
	it("keeps contextual modules below the continuation complexity budget", async () => {
		const source_files = (await readdir(source_directory)).filter((path) =>
			path.endsWith(".ts"),
		);
		const line_counts = await Promise.all(
			source_files.map(async (path) => ({
				lines: (await readFile(resolve(source_directory, path), "utf8")).split("\n").length,
				path,
			})),
		);

		expect(line_counts.filter(({ lines }) => lines >= 700)).toEqual([]);
		expect(line_counts.find(({ path }) => path === "repository.ts")?.lines).toBeLessThan(400);
	});

	it("keeps persistence decoding schema-owned and assertion-free", async () => {
		const source_files = (await readdir(source_directory)).filter((path) =>
			path.endsWith(".ts"),
		);
		const source = (
			await Promise.all(
				source_files.map((path) => readFile(resolve(source_directory, path), "utf8")),
			)
		).join("\n");

		expect(source).not.toContain("JSON.parse");
		expect(source).not.toMatch(/\w+![.;,)\]]/u);
		expect(source).toContain("Schema.fromJsonString");
	});
});
