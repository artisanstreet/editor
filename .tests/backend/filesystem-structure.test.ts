import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const node_source = resolve("modules/backend/src/filesystem/node");
const ReadSource = (file_name: string) => readFileSync(resolve(node_source, file_name), "utf8");
const line_count = (file_name: string) => ReadSource(file_name).split(/\r?\n/u).length;

describe("Node filesystem structure", () => {
	it("keeps the adapter and replacement transaction independently reviewable", () => {
		expect(line_count("service.ts")).toBeLessThan(800);
		expect(line_count("replacement.ts")).toBeLessThan(700);
		expect(line_count("context.ts")).toBeLessThan(100);
	});

	it("owns conditional replacement in its scoped Effect service", () => {
		const adapter = ReadSource("service.ts");
		const replacement = ReadSource("replacement.ts");

		expect(adapter).not.toContain("const ReplaceRegularFile");
		expect(adapter).not.toContain("const FinalizeRegularFileReplacement");
		expect(replacement).toContain("yield* NodeReplacementContext");
		expect(replacement).not.toMatch(/\bPromise\.(?:all|race|resolve|reject)\b/u);
		expect(replacement).not.toMatch(/\bEffect\.run(?:Fork|Promise|Sync)\b/u);
		expect(replacement).not.toMatch(/\bset(?:Timeout|Interval)\b/u);
	});
});
