import { readdir, readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const boundary_sources = [
	"modules/backend/src/model-behaviour/config-files.ts",
	"modules/backend/src/model-behaviour/codex-probe.ts",
	"modules/backend/src/model-favorites/service.ts",
	"modules/backend/src/orchestration/internal/persisted-graph-codecs.ts",
	"modules/backend/src/git/service.ts",
] as const;

describe("model file structure", () => {
	it("lets model domain directories provide filename context", async () => {
		const behaviour_files = await readdir("modules/backend/src/model-behaviour");
		const favorite_files = await readdir("modules/backend/src/model-favorites");

		expect(behaviour_files).not.toContain("model-behaviour-config-files.ts");
		expect(behaviour_files).not.toContain("model-behaviour-provider.ts");
		expect(behaviour_files).not.toContain("model-behaviour-registry.ts");
		expect(behaviour_files).not.toContain("model-behaviour-repository.ts");
		expect(behaviour_files).not.toContain("model-behaviour-service.ts");
		expect(behaviour_files).not.toContain("model-behaviour-value.ts");
		expect(favorite_files).not.toContain("model-favorites-service.ts");
	});

	it.each(boundary_sources)("%s has checked JSON and optional-value boundaries", async (path) => {
		const source = await readFile(path, "utf8");

		expect(source).not.toMatch(/\bJSON\.parse\s*\(/);
		expect(source).not.toMatch(/(?:\w|\]|\))!(?=[.[,;)\]])/);
	});
});
