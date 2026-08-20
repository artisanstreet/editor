import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("Forge failure overlay loading", () => {
	it("paints recovery UI before concurrent discovery and diagnostic reads settle", () => {
		const source = readFileSync(
			resolve("modules/frontend/src/routes/components/forge-connection-overlay.svelte"),
			"utf8",
		);

		expect(source).toContain("const LoadFailureContext = (generation: number)");
		expect(source).toContain("[DiscoverForge.pipe(Effect.option), read_diagnostics]");
		expect(source).toContain('{ concurrency: "unbounded" }');
		expect(source).toContain("LoadFailureContext(generation).pipe(Effect.forkScoped)");
		expect(source).toContain("generation !== failure_load_generation");
		expect(source).not.toContain("yield* LoadDiscovery");
		expect(source).not.toContain("journal = yield* read_diagnostics");
	});
});
