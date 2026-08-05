import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace_root = resolve(import.meta.dirname, "../..");

const read_json_version = async (path: string) => {
	const parsed = JSON.parse(await readFile(resolve(workspace_root, path), "utf8")) as {
		readonly version?: string;
	};
	return parsed.version;
};

/**
 * The release planner refuses a requested version that differs from the Cargo
 * workspace version, but the npm manifests carry their own copies and nothing
 * before release time noticed drift. This gate fails the ordinary test run
 * instead, so a bump can never ship half-applied.
 */
describe("workspace version sync", () => {
	it("declares one version across the Cargo workspace and npm manifests", async () => {
		const cargo = await readFile(resolve(workspace_root, "Cargo.toml"), "utf8");
		const cargo_version = cargo.match(/^version = "([^"]+)"/m)?.[1];

		expect(cargo_version).toBeDefined();
		expect(await read_json_version("package.json")).toBe(cargo_version);
		expect(await read_json_version("modules/installer/package.json")).toBe(cargo_version);
	});
});
