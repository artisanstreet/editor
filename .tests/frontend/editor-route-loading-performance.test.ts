import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const route_source = readFileSync(
	resolve(process.cwd(), "modules/frontend/src/routes/components/editor-route.svelte"),
	"utf8",
);

describe("editor route loading", () => {
	it("loads recent changes only for the empty editor route", () => {
		expect(route_source).toContain(
			"const LoadRecentChanges = (\n\t\tpath: string | undefined,\n\t\ttarget_thread_id: string | undefined,\n\t\ttarget_workspace_id: string | undefined,",
		);
		expect(route_source).toMatch(
			/if \(\n\s*path !== undefined \|\|[\s\S]*?\n\s*\)\n\s*return;[\s\S]*?client\n\s*\.ListWorkspaceChanges\(/,
		);
		expect(route_source).toContain(
			"LoadRecentChanges(active_path, thread_id, workspace_id).pipe(Effect.forkScoped)",
		);
	});
});
