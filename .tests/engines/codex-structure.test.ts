import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const codex_root = fileURLToPath(new URL("../../modules/engines/src/codex", import.meta.url));

const ReadTypeScriptFiles = (directory: string): Promise<ReadonlyArray<string>> =>
	readdir(directory, { withFileTypes: true }).then((entries) =>
		Promise.all(
			entries.map((entry) => {
				const path = join(directory, entry.name);

				return entry.isDirectory()
					? ReadTypeScriptFiles(path)
					: Promise.resolve(entry.name.endsWith(".ts") ? [path] : []);
			}),
		).then((paths) => paths.flat()),
	);

describe("Codex adapter structure", () => {
	it("uses contextual filenames and bounded modules", async () => {
		const files = await ReadTypeScriptFiles(codex_root);
		const prefixed = files.filter((path) => /[\\/]codex-[^\\/]+\.ts$/.test(path));
		const oversized = (
			await Promise.all(
				files.map(async (path) => ({
					lines: (await readFile(path, "utf8")).split(/\r?\n/u).length,
					path,
				})),
			)
		).filter(({ lines }) => lines >= 1_000);

		expect(prefixed).toEqual([]);
		expect(oversized).toEqual([]);
	});
});
