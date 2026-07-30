import { readdir, readFile } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const routines_root = join(
	dirname(fileURLToPath(import.meta.url)),
	"../../modules/backend/src/marketplace/routines",
);

describe("marketplace routine structure", () => {
	it("keeps directory context out of filenames and production modules bounded", async () => {
		const files = (await readdir(routines_root)).filter((file) => file.endsWith(".ts"));

		expect(
			files.filter((file) => /^routine-|^production-routine-/.test(basename(file))),
		).toEqual([]);

		const sizes = await Promise.all(
			files.map(async (file) => ({
				file,
				lines: (await readFile(join(routines_root, file), "utf8")).split(/\r?\n/).length,
			})),
		);

		expect(sizes.filter(({ lines }) => lines >= 1_000)).toEqual([]);
	});
});
