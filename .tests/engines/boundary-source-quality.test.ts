import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const boundary_sources = [
	"modules/engines/src/process/jsonl.ts",
	"modules/engines/src/claude/probe.ts",
	"modules/engines/src/claude/sdk-engine.ts",
	"modules/engines/src/claude/usage.ts",
	"modules/engines/src/codex/usage.ts",
] as const;

describe("engine boundary source quality", () => {
	it.each(boundary_sources)(
		"%s decodes JSON with Schema and avoids non-null assertions",
		async (path) => {
			const source = await readFile(path, "utf8");

			expect(source).not.toMatch(/\bJSON\.parse\s*\(/);
			expect(source).not.toMatch(/(?:\w|\])!(?=[.[,;)\]])/);
		},
	);
});
