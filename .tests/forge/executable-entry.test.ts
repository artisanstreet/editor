import { describe, expect, it } from "vitest";

import { ResolveForgeSeaCacheRoot } from "../../modules/forge/src/executable-runtime";

describe("Forge SEA executable entry", () => {
	it("prefers an explicit private runtime cache root", () => {
		expect(
			ResolveForgeSeaCacheRoot({ ARTISAN_SEA_CACHE_ROOT: "C:\\Artisan\\cache" }, "C:\\Temp"),
		).toContain("Artisan\\cache");
	});

	it("uses the local application data root on installed Windows", () => {
		expect(
			ResolveForgeSeaCacheRoot({ LOCALAPPDATA: "C:\\Users\\artisan\\AppData\\Local" }),
		).toContain("Artisan\\Forge\\runtime");
	});
});
