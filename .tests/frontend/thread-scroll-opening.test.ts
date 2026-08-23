import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const repository_root = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(repository_root, path), "utf8");

describe("thread scroll opening", () => {
	it("opens every rendered thread at the latest content without scroll memory", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const runtime = Read("modules/frontend/src/lib/runtime/browser-frontend-runtime.ts");
		const memory_path = resolve(
			repository_root,
			"modules/frontend/src/lib/conversation/scroll-memory.ts",
		);
		const positioning = workspace.match(
			/const PositionLoadedThread[\s\S]*?yield\* Effect\.addFinalizer/u,
		)?.[0];
		const bottom_assignment = positioning?.indexOf("viewport.scrollTop =") ?? -1;
		const positioned_latch = positioning?.indexOf("positioned = true") ?? -1;

		expect(existsSync(memory_path)).toBe(false);
		expect(runtime).not.toContain("ThreadScrollMemory");
		expect(workspace).not.toContain("RememberScrollPosition");
		expect(positioning).toBeDefined();
		expect(positioning).toContain("if (view_state === undefined) return;");
		expect(positioning).toContain("yield* Effect.promise(() => tick());");
		expect(positioning).toContain("if (viewport === null || positioned) return;");
		expect(positioning).toContain("viewport.scrollTop = ConversationBottomScrollTop(");
		expect(positioning).not.toContain("scrollTo({");
		expect(bottom_assignment).toBeGreaterThan(-1);
		expect(bottom_assignment).toBeLessThan(positioned_latch);
	});

	it("keeps the opening pinned while asynchronous transcript surfaces finish rendering", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		/**
		 * SER observes arguments at a yield site, not state reads hidden inside a
		 * prebuilt Effect. Without these arguments both setup programs execute once
		 * against null DOM bindings and never attach after mount.
		 */
		expect(workspace).toContain(
			"yield* SyncTranscriptSizeObserver(transcript_content, viewport);",
		);
		expect(workspace).toContain("yield* SyncFollowListeners(viewport);");
		expect(workspace).toContain("observer.observe(content);");
		expect(workspace).toContain("observer.observe(current_viewport);");
		expect(workspace).toContain("top: ConversationBottomScrollTop(");
		expect(workspace).not.toContain("yield* SyncTranscriptSizeObserver;");
		expect(workspace).not.toContain("yield* SyncFollowListeners;");
	});

	it("lands an accepted-message anchor before animating its visual travel", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const anchor_layout = workspace.match(
			/const UpdateAnchorLayout[\s\S]*?const ScheduleAnchorLayout/u,
		)?.[0];

		expect(workspace).toContain(
			"yield* UpdateAnchorLayout(true).pipe(Effect.forkIn(anchor_scope));",
		);
		expect(anchor_layout).toBeDefined();
		expect(anchor_layout).toContain('behavior: "auto"');
		expect(anchor_layout).not.toContain('behavior: "smooth"');
		expect(anchor_layout).toContain("GlideAnchorCorrection(");
		expect(anchor_layout?.indexOf("yield* ArmAnchorScroll(")).toBeLessThan(
			anchor_layout?.indexOf("viewport.scrollTo({") ?? -1,
		);
	});
});
