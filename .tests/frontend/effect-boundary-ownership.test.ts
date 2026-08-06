import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(path, "utf8");

describe("frontend Effect boundary ownership", () => {
	it.each([
		"modules/frontend/src/routes/components/conversation-work-session.svelte",
		"modules/frontend/src/lib/components/activity/vertical-calendar-activity-grid.svelte",
	])("%s owns observers in the component scope", (path) => {
		const source = Read(path);

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain("Effect.acquireRelease");
		expect(source).toContain("MakeScopedAttachmentRunner");
		expect(source).not.toContain("Queue.unbounded");
		expect(source).not.toContain("onMount(");
		expect(source).not.toMatch(/Effect\.run(?:Fork|Promise|Sync)/);
	});
});
