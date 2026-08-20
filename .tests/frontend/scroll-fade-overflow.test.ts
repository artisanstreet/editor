import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("scroll fade overflow policy", () => {
	it("keeps the bottom edge clear while its scroll timeline is inactive", () => {
		const animations = read("modules/frontend/src/lib/styles/animations.css");
		const utilities = read("modules/frontend/src/lib/styles/utilities.css");
		const end_property = animations.match(/@property --docs-scroll-fade-end\s*\{[^}]*\}/)?.[0];

		expect(end_property).toContain("initial-value: 0px");
		expect(animations).toContain("--docs-scroll-fade-end: var(--docs-scroll-fade-size)");
		expect(utilities).toContain("animation-timeline: scroll(self block), scroll(self block)");
	});
});
