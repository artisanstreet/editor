import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const data_root = fileURLToPath(new URL("../../modules/data", import.meta.url));

const catalog_paths = readdirSync(data_root, { encoding: "utf8", recursive: true })
	.filter((path) => path.endsWith(".json") && path !== "package.json")
	.toSorted();

const read_catalog = (path: string): ReadonlyArray<string> =>
	JSON.parse(readFileSync(join(data_root, path), "utf8")) as ReadonlyArray<string>;

describe("curated product data", () => {
	it("keeps every shipped catalog deliberate and bounded", () => {
		expect(catalog_paths.length).toBeGreaterThan(0);

		for (const path of catalog_paths) {
			const catalog = read_catalog(path);

			expect(Array.isArray(catalog), path).toBe(true);
			expect(catalog.length, path).toBeLessThanOrEqual(100);
			expect(new Set(catalog).size, path).toBe(catalog.length);
			expect(
				catalog.every((entry) => entry === entry.normalize("NFC")),
				path,
			).toBe(true);
		}
	});

	it("keeps name catalogs to single-word names", () => {
		for (const path of catalog_paths.filter((path) => path.startsWith("names"))) {
			const catalog = read_catalog(path);

			expect(
				catalog.every((name) => /^\p{L}+$/u.test(name)),
				path,
			).toBe(true);
		}
	});

	it("preserves explicitly curated names", () => {
		expect(read_catalog("names/british-females.json")).toContain("Esmebeth");
		expect(read_catalog("names/norwegian-females.json")).toEqual(
			expect.arrayContaining(["Elise", "Linnea", "Martha"]),
		);
		expect(read_catalog("names/norwegian-females.json")).not.toContain("Marta");
		expect(read_catalog("activity-status/thinking-words.json")).toContain("Muhammading");
	});
});
