import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const panel = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/thread-panel.svelte",
	),
	"utf8",
);

describe("project switch draft handoff", () => {
	it("moves the active composer document before changing workspace routes", () => {
		expect(panel).toContain("thread_id ?? new_thread_draft_key(workspace_id)");
		expect(panel).toContain("new_thread_draft_key(next.project_id)");
		expect(panel).toContain("composer_drafts.Move(current_draft_key, next_draft_key)");
		expect(panel.indexOf("composer_drafts.Move")).toBeLessThan(
			panel.indexOf("navigation.Navigate(WorkspaceRoutePath(next.project_id))"),
		);
	});
});
