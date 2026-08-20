import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const read = (path: string) => readFileSync(resolve(path), "utf8");

describe("sidebar engine authentication", () => {
	it("keeps every unauthenticated provider visible with a foreground name", () => {
		const usage = read("modules/frontend/src/routes/components/sidebar-engine-usage.svelte");

		expect(usage).toContain('readonly kind: "unauthenticated";');
		expect(usage).toContain('report.authentication === "unauthenticated"');
		expect(usage).toContain(': { engine_id, kind: "unauthenticated" };');
		expect(usage).toContain('{:else if row.kind === "unauthenticated"}');
		expect(usage).toContain(
			'font-medium text-foreground">{EngineDisplayName(row.engine_id)}</span>',
		);
		expect(usage).toContain("{EngineDisplayName(row.engine_id)} is not authenticated");
	});
});
