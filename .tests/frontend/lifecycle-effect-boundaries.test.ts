import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const ReadSource = (path: string) => readFile(new URL(`../../${path}`, import.meta.url), "utf8");

const ComponentPaths = [
	"modules/frontend/src/routes/components/conversation-message.sv",
	"modules/frontend/src/routes/components/shader-dev-panel.sv",
	"modules/frontend/src/routes/components/paper-god-rays.sv",
] as const;

const DropdownCallerPaths = [
	"modules/frontend/src/routes/components/thread-panel.sv",
	"modules/frontend/src/routes/components/model-selector/policy-controls.sv",
	"modules/frontend/src/routes/components/model-selector/compaction-control.sv",
] as const;

describe("frontend browser lifecycle ownership", () => {
	it.each(ComponentPaths)("%s is an SER component with scoped resources", async (path) => {
		const source = await ReadSource(path);

		expect(source).toContain('<script lang="ts" effect>');
		expect(source).toContain("Effect.acquireRelease");
		expect(source).not.toMatch(/\bEffect\.runFork\b|\brunFork\b/);
		expect(source).not.toMatch(/\bas unknown as\b|!\./);
		expect(source.split("\n").length).toBeLessThan(800);
	});

	it("owns dropdown attachments with a component-scoped queue and fibers", async () => {
		const source = await ReadSource(
			"modules/frontend/src/lib/components/dropdown-highlight.ts",
		);

		expect(source).toContain("const AcquireHighlight");
		expect(source).toContain("Effect.acquireRelease");
		expect(source).toContain("Queue.unbounded<HighlightCommand>()");
		expect(source).toContain("Effect.forkScoped");
		expect(source).toContain("Fiber.interrupt(fiber)");
		expect(source).not.toMatch(
			/\bEffect\.run(?:Sync|Fork)\b|\brun(?:Sync|Fork)\b|\bScope\.(?:make|close)\b/,
		);
		expect(source).not.toMatch(/\bas unknown as\b|!\./);
	});

	it.each(DropdownCallerPaths)(
		"%s acquires its dropdown attachment through SER",
		async (path) => {
			const source = await ReadSource(path);

			expect(source).toContain('<script lang="ts" effect>');
			expect(source).toContain("const FollowHighlight = yield* MakeFollowHighlight");
		},
	);
});
