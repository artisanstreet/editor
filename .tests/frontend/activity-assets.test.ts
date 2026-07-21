import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

const asset_root = join(process.cwd(), "modules/frontend/static/activity");
const component_path = join(
	process.cwd(),
	"modules/frontend/src/routes/components/activity-status.sv",
);

describe("Artisan working sprite", () => {
	it("has a valid RGBA bitmap and internally consistent manifest", () => {
		const image = readFileSync(join(asset_root, "artisan-working-sprite.png"));
		const manifest = JSON.parse(
			readFileSync(join(asset_root, "artisan-working-sprite.json"), "utf8"),
		) as Record<string, unknown>;

		expect(image.subarray(0, 8)).toEqual(
			Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
		);
		expect(image.readUInt32BE(16)).toBe(256);
		expect(image.readUInt32BE(20)).toBe(256);
		expect(image[25]).toBe(6);
		expect(manifest).toMatchObject({
			asset: "/activity/artisan-working-sprite.png",
			columns: 2,
			duration_ms: 1600,
			frame_count: 4,
			frame_height: 128,
			frame_rate_fps: 2.5,
			frame_width: 128,
			loop: true,
			reduced_motion_frame: 0,
			rows: 2,
		});
	});

	it("uses crisp integer sprite scaling and an explicit reduced-motion still", () => {
		const source = readFileSync(component_path, "utf8");

		expect(source).toContain("image-rendering: pixelated");
		expect(source).toContain("@media (prefers-reduced-motion: reduce)");
		expect(source).toMatch(/animation:\s*none/);
		expect(source).toContain('aria-label="Artisan is working"');
	});
});
