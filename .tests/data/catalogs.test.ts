import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

type CatalogEntry = Readonly<{
	value: string;
	weight: number;
}>;

const data_root = fileURLToPath(new URL("../../modules/data", import.meta.url));

const catalog_paths = readdirSync(data_root, { encoding: "utf8", recursive: true })
	.filter((path) => path.endsWith(".json") && path !== "package.json")
	.toSorted();

const allowed_weights = new Set([1, 2, 4, 6, 8]);

const read_catalog = (path: string): ReadonlyArray<CatalogEntry> =>
	JSON.parse(readFileSync(join(data_root, path), "utf8")) as ReadonlyArray<CatalogEntry>;

const find_entry = (path: string, value: string): CatalogEntry | undefined =>
	read_catalog(path).find((entry) => entry.value === value);

describe("curated product data", () => {
	it("keeps every shipped catalog deliberate, weighted, and bounded", () => {
		expect(catalog_paths.length).toBeGreaterThan(0);

		for (const path of catalog_paths) {
			const catalog = read_catalog(path);
			const values = catalog.map((entry) => entry.value);

			expect(Array.isArray(catalog), path).toBe(true);
			expect(catalog.length, path).toBeLessThanOrEqual(100);
			expect(new Set(values).size, path).toBe(catalog.length);

			for (const entry of catalog) {
				expect(Object.keys(entry).toSorted(), path).toEqual(["value", "weight"]);
				expect(entry.value, path).toBe(entry.value.normalize("NFC"));
				expect(Number.isSafeInteger(entry.weight), path).toBe(true);
				expect(allowed_weights.has(entry.weight), path).toBe(true);
			}
		}
	});

	it("keeps name catalogs to single-word names", () => {
		for (const path of catalog_paths.filter((path) => path.startsWith("names"))) {
			const catalog = read_catalog(path);

			expect(
				catalog.every(({ value }) => /^\p{L}+$/u.test(value)),
				path,
			).toBe(true);
		}
	});

	it("preserves explicitly curated rarity", () => {
		expect(find_entry("names/british-females.json", "Esmebeth")?.weight).toBe(8);
		expect(find_entry("names/norwegian-females.json", "Mina")?.weight).toBe(8);
		expect(read_catalog("names/norwegian-females.json").map(({ value }) => value)).toEqual(
			expect.arrayContaining(["Elise", "Linnea", "Martha"]),
		);
		expect(find_entry("names/norwegian-females.json", "Marta")).toBeUndefined();
		expect(find_entry("activity-status/thinking-words.json", "Muhammading")?.weight).toBe(1);
		expect(
			read_catalog("activity-status/thinking-words.json")
				.filter(({ value }) => value !== "Muhammading")
				.every(({ weight }) => weight >= 4),
		).toBe(true);
	});
});
