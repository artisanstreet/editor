import { describe, expect, it } from "vitest";

import { release_lanes, transport_asset_names } from "../../build/release/policy.ts";

describe("release policy", () => {
	it("publishes only qualified product lanes", () => {
		expect(
			release_lanes.filter((lane) => lane.state === "supported").map((lane) => lane.id),
		).toEqual(["windows-x64"]);
		expect(release_lanes.filter((lane) => lane.state === "planned")).toHaveLength(6);
		expect(
			release_lanes.every((lane) => lane.state === "supported" || Boolean(lane.reason)),
		).toBe(true);
	});

	it("matches landing transport asset names exactly", () => {
		expect(transport_asset_names("1.2.3")).toContain("artisan-bootstrap-windows-x64.exe");
		expect(transport_asset_names("1.2.3")).toContain(
			"artisan-bootstrap-windows-x64.exe.sha256",
		);
		expect(transport_asset_names("1.2.3")).toContain("release-manifest.json");
		expect(transport_asset_names("1.2.3")).toContain("release-manifest.sig");
	});
});
