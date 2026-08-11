import { readFileSync } from "node:fs";

import {
	select_validation_area,
	select_validation_scripts,
	validation_areas,
} from "../../.scripts/validate/runner.ts";
import { describe, expect, it } from "vitest";

const root = new URL("../..", import.meta.url);

describe("validation entrypoint", () => {
	it("keeps the canonical full gate and every focused area discoverable from package scripts", () => {
		const manifest = JSON.parse(readFileSync(new URL("package.json", root), "utf8")) as {
			scripts: Record<string, string>;
		};

		expect(manifest.scripts.validate).toBe("node .scripts/validate/runner.ts");
		expect(manifest.scripts["validate:full"]).toBe("node .scripts/validate/runner.ts full");

		for (const area of Object.keys(validation_areas)) {
			expect(manifest.scripts[`validate:${area}`]).toBe(
				`node .scripts/validate/runner.ts ${area}`,
			);
		}
	});

	it("resolves focused areas to their small, ordered gates", () => {
		expect(select_validation_area("frontend")).toEqual([
			"format:check:frontend",
			"lint:frontend",
			"check:frontend",
			"test:frontend",
		]);
		expect(select_validation_area("native")).toEqual(["check:native", "test:native"]);
		expect(select_validation_area()).toEqual(validation_areas.full);
		expect(select_validation_scripts(["frontend", "native"])).toEqual([
			...validation_areas.frontend,
			...validation_areas.native,
		]);
		expect(select_validation_scripts(["native", "native"])).toEqual(validation_areas.native);
		expect(() => select_validation_area("unknown")).toThrow("unknown validation area");
	});

	it("exposes an arbitrary focused Vitest target command", () => {
		const manifest = JSON.parse(readFileSync(new URL("package.json", root), "utf8")) as {
			scripts: Record<string, string>;
		};

		expect(manifest.scripts["test:focus"]).toBe("vitest run --config .config/vitest.config.ts");
	});
});
