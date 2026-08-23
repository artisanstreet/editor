import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { DesktopAppIconsAvailable } from "../../modules/frontend/src/lib/browser/desktop-app-icon";

const root = new URL("../..", import.meta.url);

describe("desktop app icon settings", () => {
	it("limits runtime switching to the bundled app origin", () => {
		expect(DesktopAppIconsAvailable("artisan:")).toBe(true);
		expect(DesktopAppIconsAvailable("https:")).toBe(false);
		expect(DesktopAppIconsAvailable("http:")).toBe(false);
	});

	it("offers the foreground-gradient default and plastic-jaw alternate", () => {
		const appearance = readFileSync(
			new URL("modules/frontend/src/routes/components/settings/appearance.svelte", root),
			"utf8",
		);
		expect(appearance).toContain('label: "Plastic + jaw shading"');
		expect(appearance).toContain('value: "plastic-jaw-shading"');
		expect(appearance).toContain('label: "Foreground plastic + gradient symbol"');
		expect(appearance).toContain('value: "foreground-gradient-symbol"');
		expect(appearance).toContain("yield* SelectDesktopAppIcon(icon)");
	});
});
