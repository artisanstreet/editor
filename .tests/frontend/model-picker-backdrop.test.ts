import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("model picker backdrop composition", () => {
	it("samples the backdrop on the animated popover rather than below its transform", () => {
		const picker = Read("modules/frontend/src/routes/components/model-selector/view.svelte");
		const surface = Read("modules/frontend/src/routes/components/shader-glass-surface.svelte");
		const utilities = Read("modules/frontend/src/lib/styles/utilities.css");

		expect(picker).toContain('class="t-dropdown shader-glass-backdrop');
		expect(picker).toContain('data-strength="strong"');
		expect(picker).toContain("use_backdrop_filter={false}");
		expect(surface).toContain("class:shader-glass-backdrop={use_backdrop_filter}");
		expect(utilities).toContain("@utility shader-glass-backdrop");
		expect(utilities).toContain('&[data-strength="strong"]');
	});
});
