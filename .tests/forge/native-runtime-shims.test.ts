import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

describe("Forge native runtime shims", () => {
	it.each([
		["modules/desktop/src/node-pty-shim.ts", "LoadNodePty"],
		["modules/desktop/src/koffi-shim.ts", "LoadKoffi"],
	])("defers %s loading until first use", (path, loader) => {
		const source = readFileSync(path, "utf8");

		expect(source).toContain(`const ${loader} = () =>`);
		expect(source).toContain("process.env.ARTISAN_NATIVE_RUNTIME");
		expect(source.indexOf("require(")).toBeGreaterThan(source.indexOf(`const ${loader}`));
	});
});
