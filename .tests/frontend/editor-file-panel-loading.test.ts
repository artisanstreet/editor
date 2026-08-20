import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const panel_source = readFileSync(
	resolve(process.cwd(), "modules/frontend/src/routes/components/editor-file-panel.svelte"),
	"utf8",
);

describe("editor file panel directory loading", () => {
	it("forks and coalesces directory listings without letting stale work publish", () => {
		expect(panel_source).toContain("Effect.forkScoped");
		expect(panel_source).toContain('Effect.timeoutOption("10 seconds")');
		expect(panel_source).toContain("const active_directory_requests = yield* Ref.make");
		expect(panel_source).toContain("const request_key = `${generation}:${parent}`");
		expect(panel_source).toContain("if (!claimed) return;");
		expect(panel_source).toContain(
			"if (generation !== (yield* Ref.get(workspace_generation))) return;",
		);
		expect(panel_source).toContain("if (workspace_id !== loaded_workspace_id)");
		expect(panel_source).toContain(
			"yield* RequestDirectory(workspace_tree_root, workspace_id);",
		);
		expect(panel_source).not.toContain("let directory_requests");
	});
});
