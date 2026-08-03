import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const forge_host_path = fileURLToPath(
	new URL("../../modules/forge/src/forge-host.ts", import.meta.url),
);

describe("Forge thread-title activation", () => {
	it("waits for automatic metadata catch-up before accepting browser sessions", async () => {
		const source = await readFile(forge_host_path, "utf8");
		const host = source.slice(source.indexOf("const MakeForgeHost"));
		const resolve_refinement = host.indexOf(
			"const metadata_refinement = yield* ThreadMetadataRefinementCoordinator;",
		);
		const wait_for_refinement = host.indexOf("yield* metadata_refinement.WaitForIdle;");
		const start_http = host.indexOf("start_forge_http(config, authority)");
		const bind_transport = host.indexOf("transport_binding.Bind({");

		expect(resolve_refinement).toBeGreaterThanOrEqual(0);
		expect(wait_for_refinement).toBeGreaterThan(resolve_refinement);
		expect(start_http).toBeGreaterThan(wait_for_refinement);
		expect(bind_transport).toBeGreaterThan(start_http);
	});
});
