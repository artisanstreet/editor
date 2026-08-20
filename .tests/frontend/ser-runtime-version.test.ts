import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

describe("Svelte Effect Runtime version", () => {
	it("pins the run-scope release and its supply-chain exception", () => {
		const frontend_package = JSON.parse(
			readFileSync(resolve("modules/frontend/package.json"), "utf8"),
		) as { readonly dependencies?: Readonly<Record<string, string>> };
		const lockfile = readFileSync(resolve("pnpm-lock.yaml"), "utf8");
		const workspace = readFileSync(resolve("pnpm-workspace.yaml"), "utf8");

		expect(frontend_package.dependencies?.["svelte-effect-runtime"]).toBe("4.2.4");
		expect(lockfile).toContain("svelte-effect-runtime@4.2.4");
		expect(lockfile).not.toContain("svelte-effect-runtime@4.0.0");
		expect(workspace).toContain("svelte-effect-runtime@4.2.4");
		/**
		 * 4.2.3 shipped the superseded-cell eviction; 4.2.4 scopes forked work to
		 * the reactive run rather than the component, so a rerun no longer leaves
		 * its work alive until the component is destroyed. Both are retention
		 * fixes, and a long-lived route reruns constantly — this pin exists to
		 * stop the app sliding back below either. No local patch remains.
		 */
		expect(workspace).not.toContain("patches/svelte-effect-runtime");
	});
});
