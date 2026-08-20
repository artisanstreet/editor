import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const Footer = "modules/frontend/src/routes/components/conversation-turn-footer.svelte";

describe("conversation turn footer performance", () => {
	it("does not retain a settled-history clock and refreshes age on interaction", () => {
		const source = readFileSync(resolve(workspace, Footer), "utf8");

		expect(source).not.toContain("Effect.sleep(");
		expect(source).not.toContain("while (true)");
		expect(source).not.toContain("Effect.forever(");
		expect(source).not.toContain("KeepClockCurrent");
		expect(source).toContain("const RefreshAge = Effect.gen(function* ()");
		expect(source).toContain("now = yield* Clock.currentTimeMillis");
		expect(source).toContain("onmouseenter={yield* RefreshAge}");
		expect(source).toContain("onfocusin={yield* RefreshAge}");
	});
});
