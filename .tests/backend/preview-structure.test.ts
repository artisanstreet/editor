import { readdir, readFile } from "node:fs/promises";
import { basename, join } from "node:path";

import { describe, expect, it } from "vitest";

const preview_root = join(process.cwd(), "modules", "backend", "src", "preview");

describe("preview source structure", () => {
	it("uses contextual filenames and bounded modules", async () => {
		const filenames = (await readdir(preview_root)).filter((name) => name.endsWith(".ts"));
		expect(filenames.filter((name) => basename(name).startsWith("preview-"))).toEqual([]);

		const sizes = await Promise.all(
			filenames.map(async (name) => ({
				name,
				lines: (await readFile(join(preview_root, name), "utf8")).split(/\r?\n/u).length,
			})),
		);
		expect(sizes.filter(({ lines }) => lines >= 1_000)).toEqual([]);
		expect(sizes.find(({ name }) => name === "repository.ts")?.lines).toBeLessThan(850);
	});

	it("decodes persisted JSON through Schema and avoids unsafe non-null narrowing", async () => {
		const filenames = ["repository.ts", "storage-codec.ts"];
		const sources = await Promise.all(
			filenames.map(async (name) => ({
				name,
				source: await readFile(join(preview_root, name), "utf8"),
			})),
		);

		expect(
			sources.flatMap(({ name, source }) => (source.includes("JSON.parse") ? [name] : [])),
		).toEqual([]);
		expect(
			sources.flatMap(({ name, source }) =>
				/(?<![=!])!(?=[.,;)\]}])/u.test(source) ? [name] : [],
			),
		).toEqual([]);
	});
});
