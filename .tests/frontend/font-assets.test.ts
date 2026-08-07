import { existsSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");

describe("frontend font assets", () => {
	it("ships only font faces used by the application", () => {
		const fonts = readFileSync(
			resolve(workspace, "modules/frontend/src/lib/styles/fonts.css"),
			"utf8",
		);

		expect(fonts).toContain('font-family: "Artisan Neo";');
		expect(fonts).toContain('font-family: "Geist";');
		expect(fonts).toContain('font-family: "JetBrains Mono";');
		expect(fonts).toContain('font-family: "Cal Sans";');
		expect(fonts).toContain('font-family: "Sigurd Variable";');
		expect(fonts).not.toContain("PP Neue Montreal");
		expect(fonts).not.toMatch(/font-family: "Artisan Neo (?:Edge|Soft|Round|Grotesk|Wink)"/u);
		expect(
			existsSync(
				resolve(workspace, "modules/frontend/static/fonts/artisan-neo-variable.woff2"),
			),
		).toBe(false);
	});

	it("ships a tightly bounded Sigurd WOFF2 subset for the ARTISAN wordmark", () => {
		const sigurd_root = resolve(workspace, "modules/frontend/src/lib/assets/fonts/sigurd");
		const subset = resolve(sigurd_root, "sigurd-artisan.woff2");

		expect(existsSync(subset)).toBe(true);
		expect(existsSync(resolve(sigurd_root, "sigurd-variable.otf"))).toBe(false);
		expect(statSync(subset).size).toBeLessThanOrEqual(24 * 1_024);
		expect(
			readFileSync(resolve(workspace, "modules/frontend/src/lib/styles/fonts.css"), "utf8"),
		).toContain('src: url("../assets/fonts/sigurd/sigurd-artisan.woff2") format("woff2");');
	});
});
