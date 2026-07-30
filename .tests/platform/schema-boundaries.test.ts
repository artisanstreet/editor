import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../..");
const platform_boundary_files = [
	"modules/bootstrap/src/workflow.ts",
	"modules/bootstrap/src/node-runtime.ts",
	"modules/cli/src/node-instance-store.ts",
	"modules/cli/src/node-distribution-runtime.ts",
	"modules/cli/src/node-launcher.ts",
	"modules/forge/src/state.ts",
	"modules/forge/src/instance-registry.ts",
	"modules/forge/src/http-host.ts",
	"modules/forge/src/control-authority.ts",
	"modules/forge/src/database-lease.ts",
	"modules/desktop/src/renderer-host.ts",
	"modules/distribution/src/activation.ts",
	"modules/distribution/src/verification.ts",
	"modules/distribution/src/release-configuration.ts",
	"modules/distribution/src/installation-store.ts",
	"modules/distribution/src/node-release-adapters.ts",
	"modules/distribution/src/artifact-selection.ts",
] as const;

describe("platform boundary source quality", () => {
	it("uses Schema JSON codecs and validated narrowing", async () => {
		for (const path of platform_boundary_files) {
			const source = await readFile(resolve(workspace_root, path), "utf8");
			expect(source, `${path} contains raw JSON parsing`).not.toContain("JSON.parse");
			expect(source, `${path} contains a non-null assertion`).not.toMatch(
				/[\w)\]]!\s*[.[,;)]/u,
			);
		}
	});
});
