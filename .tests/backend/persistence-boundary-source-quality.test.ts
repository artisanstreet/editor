import { readFile } from "node:fs/promises";

import { describe, expect, it } from "vitest";

const persistence_boundary_sources = [
	"modules/backend/src/persistence/journal-store.ts",
	"modules/backend/src/persistence/projection-rebuild-service.ts",
	"modules/backend/src/persistence/transcript-read-model.ts",
	"modules/backend/src/persistence/orchestration/repository.ts",
	"modules/backend/src/persistence/orchestration/outbox.ts",
	"modules/backend/src/persistence/orchestration/acceptance.ts",
	"modules/backend/src/persistence/orchestration/storage-codec.ts",
	"modules/backend/src/threads/thread-retention-policy.ts",
	"modules/backend/src/threads/thread-project-affinity-repository.ts",
	"modules/backend/src/threads/thread-project-affinity-coordinator.ts",
	"modules/backend/src/threads/thread-metadata-repository.ts",
	"modules/backend/src/threads/thread-metadata-refinement-coordinator.ts",
	"modules/backend/src/threads/thread-erasure.ts",
	"modules/backend/src/threads/internal/thread-projection.ts",
] as const;

describe("backend persistence boundary source quality", () => {
	it.each(persistence_boundary_sources)(
		"%s uses Schema JSON decoding and avoids non-null assertions",
		async (path) => {
			const source = await readFile(path, "utf8");

			expect(source).not.toMatch(/\bJSON\.parse\s*\(/);
			expect(source).not.toMatch(/(?:\w|\]|\))!(?=[.[,;)\]])/);
		},
	);
});
