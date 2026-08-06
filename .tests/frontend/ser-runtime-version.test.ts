import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

describe("Svelte Effect Runtime version", () => {
	it("pins the cell-eviction release and its supply-chain exception", () => {
		const frontend_package = JSON.parse(
			readFileSync(resolve("modules/frontend/package.json"), "utf8"),
		) as { readonly dependencies?: Readonly<Record<string, string>> };
		const lockfile = readFileSync(resolve("pnpm-lock.yaml"), "utf8");
		const workspace = readFileSync(resolve("pnpm-workspace.yaml"), "utf8");

		expect(frontend_package.dependencies?.["svelte-effect-runtime"]).toBe("4.2.3");
		expect(lockfile).toContain("svelte-effect-runtime@4.2.3");
		expect(lockfile).not.toContain("svelte-effect-runtime@4.0.0");
		expect(workspace).toContain("svelte-effect-runtime@4.2.3");
		/** 4.2.3 ships the superseded-cell eviction upstream; no local patch remains. */
		expect(workspace).not.toContain("patches/svelte-effect-runtime");
	});
});
