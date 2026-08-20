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

	it("keeps the accepted-message anchor smooth after the instant thread landing", () => {
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");

		expect(workspace).toContain(
			"yield* UpdateAnchorLayout(true).pipe(Effect.forkIn(anchor_scope));",
		);
		expect(workspace).toContain(
			'behavior: globalThis.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth"',
		);
		expect(workspace).toContain("yield* ArmAnchorScroll(viewport, false);");
	});
});
