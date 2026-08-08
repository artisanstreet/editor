import { describe, expect, it } from "vitest";

import { CollectForgeSeaAssets } from "../../.scripts/build/forge-sea-assets";

const workspace_root = import.meta.dirname.replace(/[\\/]\.tests[\\/]build$/, "");

describe("Forge SEA production assets", () => {
	it("embeds the complete native and migration runtime without debug symbols", () => {
		const assets = CollectForgeSeaAssets(workspace_root);
		const paths = assets.map((asset) => asset.relative_path);

		expect(paths).toContain("native-runtime/node_modules/node-pty/package.json");
		expect(paths).toContain(
			"native-runtime/node_modules/@koromix/koffi-win32-x64/win32_x64/koffi.node",
		);
		expect(paths).toContain("native-runtime/node_modules/claude-agent-sdk/claude.exe");
		expect(paths.some((path) => path.startsWith("migrations/") && path.endsWith(".sql"))).toBe(
			true,
		);
		expect(paths.some((path) => path.endsWith(".pdb"))).toBe(false);
		expect(new Set(paths).size).toBe(paths.length);
		expect(new Set(assets.map((asset) => asset.asset_id)).size).toBe(assets.length);
	});
});
