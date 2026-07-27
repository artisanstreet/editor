import { mkdtemp, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	MAX_FORGE_LOG_BYTES,
	TruncateForgeLogIfOversized,
} from "../../modules/forge/src/log-retention";

describe("Forge log retention", () => {
	it("truncates an active detached log at the hard limit", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-forge-log-retention-"));
		const path = join(root, "forge.log");
		try {
			await writeFile(path, Buffer.alloc(MAX_FORGE_LOG_BYTES));
			await Effect.runPromise(TruncateForgeLogIfOversized(path));
			expect((await stat(path)).size).toBe(0);
		} finally {
			await rm(root, { force: true, recursive: true });
		}
	});
});
