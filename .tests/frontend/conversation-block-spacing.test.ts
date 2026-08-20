import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

describe("conversation block spacing", () => {
	it("keeps a tool chain one line from the reply that follows it", () => {
		const workspace = readFileSync(
			resolve("modules/frontend/src/routes/components/thread-workspace.svelte"),
			"utf8",
		);

		expect(workspace).toContain("group/turn relative flex flex-col gap-[1lh]");
		expect(workspace).not.toContain("group/turn relative flex flex-col gap-8");
	});
});
