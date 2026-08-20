import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("thread participant conversation", () => {
	it("filters the root transcript through the same participant boundary as workers", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toMatch(
			/MakeParticipantConversationRenderWindow\(\s*conversation_view_state,\s*inspection\?\.agent_id,/u,
		);
		expect(workspace).not.toMatch(/\bMakeConversationRenderWindow\b/u);
	});

	it("does not acknowledge root activity while the worker transcript is visible", () => {
		const rail = Read("modules/frontend/src/routes/components/thread-hover-rail.svelte");

		expect(rail).toContain("yield* orchestration.CurrentInspection");
		expect(rail).toContain("orchestration.InspectionChanges.pipe(");
		expect(rail).toMatch(
			/AcknowledgeDepartedThread\(\s*active_route_id,\s*open_thread,\s*reader_is_watching,\s*inspecting_agent,/u,
		);
		expect(rail).toContain("reader_can_acknowledge_root_conversation(");
		expect(rail).toContain("reader_activity_at: ThreadReaderActivityAt(thread)");
	});
});
