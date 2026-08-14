import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("conversation activity rail alignment", () => {
	it("centers the expanded rail on the header icon slot", () => {
		const trace = Read("modules/frontend/src/routes/components/conversation-trace.svelte");

		expect(trace).toContain("pointer-events-none absolute inset-y-0 left-0 w-4");
		/** A 2px line at 50% needs half its own width removed to share the icon's centre. */
		expect(trace).toContain("after:left-1/2 after:w-[2px] after:-translate-x-1/2");
		expect(trace).toContain('aria-hidden="true"');
	});
});
