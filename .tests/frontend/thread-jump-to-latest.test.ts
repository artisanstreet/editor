import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(path, "utf8");

describe("thread jump-to-latest affordance", () => {
	it("appears only after live-tail following stops and resumes the bottom contract", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const composer = Read("modules/frontend/src/routes/components/thread-composer.svelte");

		expect(workspace).toContain("show_jump_to_latest={!following && !anchor_scroll_active}");
		expect(workspace).toContain("anchored_user_item_id = undefined;");
		expect(workspace).toContain("end_space_height = ConversationBaseEndSpacePixels;");
		expect(workspace).toContain("ArmAnchorScroll(current_viewport, true)");
		expect(workspace).toContain('Effect.sleep("1 second")');
		expect(workspace).toContain("generation === anchor_scroll_generation");
		expect(workspace).toContain("release_anchor_scroll(current_viewport)");
		expect(composer).toContain('aria-label="Jump to latest"');
		expect(composer).toContain("<ChevronDown");
	});

	it("keeps accepted sends on the existing top-alignment path", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain("outcome.user_message_reference !== undefined");
		expect(workspace).toContain("ConversationUserMessageWithSourceReference");
		expect(workspace).toContain("ConversationAlignedScrollTop");
	});
});
