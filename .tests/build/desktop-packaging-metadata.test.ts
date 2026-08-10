import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const root = new URL("../..", import.meta.url);

describe("desktop package metadata", () => {
	it("gives electron-builder the existing high-resolution Artisan icon", () => {
		const builder_config = readFileSync(new URL(".config/desktop-builder.yml", root), "utf8");
		const icon = readFileSync(
			new URL("modules/frontend/src/lib/assets/barekey/logo-gradient.svg", root),
			"utf8",
		);

		expect(builder_config).toContain("buildResources: modules/frontend/src/lib/assets/barekey");
		expect(builder_config).toContain("icon: logo-gradient.svg");
		expect(icon).toContain('width="720" height="720"');
	});

	it("stages the metadata electron-builder reads from the desktop app manifest", () => {
		const vite_config = readFileSync(new URL(".config/desktop.vite.config.ts", root), "utf8");

		expect(vite_config).toContain('author: "Barekey"');
		expect(vite_config).toContain('description: "Artisan Editor"');
	});
});
