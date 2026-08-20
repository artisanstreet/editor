import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const forge_host_path = fileURLToPath(
	new URL("../../modules/forge/src/forge-host.ts", import.meta.url),
);

describe("Forge thread-title activation", () => {
	it("starts browser transports while automatic metadata catch-up continues in scope", async () => {
		const source = await readFile(forge_host_path, "utf8");
		const host = source.slice(source.indexOf("const MakeForgeHost"));
		const resolve_refinement = host.indexOf(
			"const metadata_refinement = yield* ThreadMetadataRefinementCoordinator;",
		);
		const start_http = host.indexOf("start_forge_http(config, authority)");
		const bind_transport = host.indexOf("transport_binding.Bind({");

		expect(resolve_refinement).toBeGreaterThanOrEqual(0);
		expect(host).not.toContain("metadata_refinement.WaitForIdle");
		expect(start_http).toBeGreaterThan(resolve_refinement);
		expect(bind_transport).toBeGreaterThan(start_http);
	});
});
