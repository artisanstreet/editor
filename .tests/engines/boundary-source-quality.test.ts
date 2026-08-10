import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const boundary_sources = [
	"modules/engines/src/process/jsonl.ts",
	"modules/engines/src/claude/probe.ts",
	"modules/engines/src/claude/cli-engine.ts",
	"modules/engines/src/claude/usage.ts",
	"modules/engines/src/codex/usage.ts",
] as const;

const provider_sdk_packages = [
	"@anthropic-ai/claude-agent-sdk",
	"@anthropic-ai/sdk",
	"@openai/codex-sdk",
] as const;

const executable_source_roots = [
	"modules/desktop/src",
	"modules/engines/src",
	"modules/forge/src",
] as const;
const executable_manifests = [
	"package.json",
	"modules/desktop/package.json",
	"modules/engines/package.json",
	"modules/forge/package.json",
] as const;

const FilesBelow = async (root: string): Promise<ReadonlyArray<string>> => {
	const files: Array<string> = [];
	for (const entry of await readdir(root, { withFileTypes: true })) {
		const path = join(root, entry.name);
		if (entry.isDirectory()) files.push(...(await FilesBelow(path)));
		else if (entry.isFile() && [".svelte", ".ts"].some((extension) => path.endsWith(extension)))
			files.push(path);
	}
	return files;
};

describe("engine boundary source quality", () => {
	it.each(boundary_sources)(
		"%s decodes JSON with Schema and avoids non-null assertions",
		async (path) => {
			const source = await readFile(path, "utf8");

			expect(source).not.toMatch(/\bJSON\.parse\s*\(/);
			expect(source).not.toMatch(/(?:\w|\])!(?=[.[,;)\]])/);
		},
	);

	it("keeps provider SDK packages out of the Forge and Editor dependency graph", async () => {
		const source_paths = (
			await Promise.all(executable_source_roots.map((root) => FilesBelow(root)))
		).flat();
		const sources = await Promise.all(source_paths.map((path) => readFile(path, "utf8")));
		const manifests = await Promise.all(
			executable_manifests.map((path) => readFile(path, "utf8")),
		);

		for (const provider_sdk_package of provider_sdk_packages) {
			expect(sources.some((source) => source.includes(provider_sdk_package))).toBe(false);
			expect(
				manifests.some((manifest) => manifest.includes(`"${provider_sdk_package}"`)),
			).toBe(false);
		}
	});
});
