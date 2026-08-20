import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const ReadSource = (path: string) => readFileSync(resolve(path), "utf8");

describe("settings layout", () => {
	it("collapses the settings rail above the page at narrow widths", () => {
		const layout = ReadSource("modules/frontend/src/routes/settings/+layout.svelte");
		const navigation = ReadSource("modules/frontend/src/routes/components/settings/nav.svelte");
		const row = ReadSource("modules/frontend/src/routes/components/settings/row.svelte");

		expect(layout).toContain("md:flex-row");
		expect(layout).toContain("overflow-y-auto");
		expect(navigation).toContain("overflow-x-auto");
		expect(row).toContain("sm:flex-row");
	});

	it("keeps engine navigation aligned with real page landmarks", () => {
		const navigation = ReadSource("modules/frontend/src/routes/components/settings/nav.svelte");
		const engine = ReadSource("modules/frontend/src/routes/components/settings/engine.svelte");

		expect(navigation).toContain('{ hash: "availability", label: "Availability" }');
		expect(navigation).toContain('{ hash: "installation", label: "Installation" }');
		expect(navigation).not.toContain('hash: "permissions"');
		expect(engine).toContain('<Section id="availability" title="Availability">');
		expect(engine).not.toContain('aria-labelledby="availability"');
	});

	it("keeps dense controls and account details usable without pointer hover", () => {
		const compaction = ReadSource(
			"modules/frontend/src/routes/components/settings/compaction-model.svelte",
		);
		const engine = ReadSource("modules/frontend/src/routes/components/settings/engine.svelte");

		expect(compaction).toContain("flex-col gap-2 sm:flex-row");
		expect(compaction).toContain("sm:h-48 sm:w-56");
		expect(engine).not.toContain("blur-[5px]");
	});
});
