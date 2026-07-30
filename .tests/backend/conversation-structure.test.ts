import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const conversation_root = fileURLToPath(
	new URL("../../modules/backend/src/conversation", import.meta.url),
);

const TypeScriptFiles = async (directory: string): Promise<ReadonlyArray<string>> => {
	const entries = await readdir(directory, { withFileTypes: true });
	const nested = await Promise.all(
		entries.map((entry) => {
			const path = `${directory}/${entry.name}`;
			return entry.isDirectory()
				? TypeScriptFiles(path)
				: Promise.resolve(entry.name.endsWith(".ts") ? [path] : []);
		}),
	);
	return nested.flat();
};

describe("conversation source structure", () => {
	it("keeps every production module below 1,000 lines and projector modules below 800", async () => {
		const files = await TypeScriptFiles(conversation_root);
		const measured = await Promise.all(
			files.map(async (file) => ({
				file,
				lines: (await readFile(file, "utf8")).split(/\r?\n/u).length,
			})),
		);
		expect(measured.filter(({ lines }) => lines >= 1_000)).toEqual([]);
		expect(
			measured.filter(
				({ file, lines }) =>
					file.replaceAll("\\", "/").includes("/projection/") && lines >= 800,
			),
		).toEqual([]);
	});

	it("keeps the projection facade declarative and small", async () => {
		const facade = await readFile(`${conversation_root}/projection-api.ts`, "utf8");
		expect(facade.split(/\r?\n/u).length).toBeLessThan(300);
		expect(facade).not.toMatch(/\bJSON\.(?:parse|stringify)\b/u);
		expect(facade).not.toMatch(/\bEffect\./u);
	});
});
