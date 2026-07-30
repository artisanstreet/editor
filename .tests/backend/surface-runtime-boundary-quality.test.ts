import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const boundary_sources = [
	"modules/backend/src/surfaces/service.ts",
	"modules/backend/src/surfaces/surface-projection.ts",
	"modules/backend/src/runtime/catalog.ts",
] as const;

describe("surface and runtime boundary quality", () => {
	it.each(boundary_sources)("%s preserves typed failures", async (path) => {
		const source = await readFile(path, "utf8");

		expect(source).not.toContain("Effect.orDie");
		expect(source).not.toContain("as unknown as");
	});

	it("constructs the surface service through its exact Context service contract", async () => {
		const source = await readFile("modules/backend/src/surfaces/service.ts", "utf8");

		expect(source).toContain("return SurfaceService.of({");
	});
});
