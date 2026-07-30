import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const safety_boundary_sources = [
	"modules/backend/src/tools/tool-invocation-repository.ts",
	"modules/backend/src/tools/tool-control-plane.ts",
	"modules/backend/src/workspace/evidence.ts",
	"modules/backend/src/workspace/changes/context.ts",
	"modules/backend/src/settings/session-defaults-service.ts",
	"modules/backend/src/preview/rich-link-service.ts",
	"modules/backend/src/guidance/service.ts",
	"modules/backend/src/git/service.ts",
	"modules/backend/src/git/read-service.ts",
	"modules/backend/src/git/parsers.ts",
	"modules/backend/src/git/mutation-lifecycle.ts",
	"modules/backend/src/marketplace/routines/production-adapters.ts",
	"modules/backend/src/model-favorites/service.ts",
	"modules/backend/src/orchestration/thread-continuation-model.ts",
	"modules/backend/src/orchestration/internal/group-start.ts",
	"modules/backend/src/orchestration/internal/graph-topology.ts",
	"modules/backend/src/orchestration/internal/graph-ledger.ts",
	"modules/backend/src/orchestration/internal/graph-context.ts",
] as const;

describe("backend safety boundary source quality", () => {
	it.each(safety_boundary_sources)(
		"%s uses Schema JSON decoding and avoids non-null assertions",
		async (path) => {
			const source = await readFile(path, "utf8");

			expect(source).not.toMatch(/\bJSON\.parse\s*\(/);
			expect(source).not.toMatch(/(?:\w|\]|\))!(?=[.[,;)\]])/);
		},
	);
});
