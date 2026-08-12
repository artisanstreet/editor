import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const root = new URL("../..", import.meta.url);

describe("desktop package metadata", () => {
	it("gives electron-builder the canonical high-resolution Artisan icon", () => {
		const builder_config = readFileSync(new URL(".config/desktop-builder.yml", root), "utf8");
		const icon = readFileSync(
			new URL("modules/frontend/src/lib/assets/barekey/artisan-app-icon.svg", root),
			"utf8",
		);

		expect(builder_config).toContain("buildResources: modules/frontend/src/lib/assets/barekey");
		expect(builder_config).toContain("icon: artisan-app-icon.ico");
		expect(icon).toContain('width="720" height="720"');
		expect(icon).toContain('id="star-rising-cutout"');
	});

	it("stages the metadata electron-builder reads from the desktop app manifest", () => {
		const vite_config = readFileSync(new URL(".config/desktop.vite.config.ts", root), "utf8");
		const build_runner = readFileSync(new URL(".scripts/build/runner.ts", root), "utf8");

		expect(vite_config).toContain('author: "Barekey"');
		expect(vite_config).toContain('description: "Artisan Editor"');
		expect(vite_config).toContain("process.env.ARTISAN_RELEASE_VERSION ?? workspace.version");
		expect(vite_config).toContain("version: desktop_version");
		expect(vite_config).not.toContain('version: "0.1.0"');
		expect(build_runner).toContain("ARTISAN_RELEASE_VERSION: version");
	});
});
