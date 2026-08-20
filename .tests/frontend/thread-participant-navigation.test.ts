import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

describe("thread participant navigation", () => {
	it("yields the roster-owned return effect for both Back and Escape", () => {
		const workspace = readFileSync(
			resolve("modules/frontend/src/routes/components/thread-workspace.svelte"),
			"utf8",
		);

		expect(workspace).toContain("onreturntoroot?: Effect.Effect<void>;");
		expect(workspace).toContain("yield* onreturntoroot;");
		expect(workspace).not.toContain("yield* onreturntoroot();");
		expect(workspace).toContain("onclick={yield* ReturnToRoot}");
		expect(workspace).toContain("onkeydown={yield* ReturnToRootOnEscape(event)}");
		expect(workspace).toContain('event.key === "Escape" && inspecting_agent');
	});
});
