import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";

const trace_path = new URL(
	"../../modules/frontend/src/routes/components/conversation-trace.svelte",
	import.meta.url,
);

describe("conversation tool-chain interaction", () => {
	it("keeps the activity rail visual-only and the group header accessible", async () => {
		const source = await readFile(trace_path, "utf8");

		expect(source).toContain('aria-hidden="true"');
		expect(source).toContain("pointer-events-none absolute inset-y-0 left-0 w-4");
		expect(source).not.toContain('aria-label="Collapse activity group"');
		expect(source).not.toContain('title="Collapse activity group"');
		expect(source).not.toContain("hover:after:bg-foreground/50");
		expect(source).toContain("aria-expanded={open}");
		expect(source).toContain("onclick={yield* ToggleGroup(segment.id)}");
	});

	it("renders tool-chain rows without hover previews or focus-only trigger behavior", async () => {
		const source = await readFile(trace_path, "utf8");

		expect(source).not.toContain("LinkPreview");
		expect(source).not.toContain("ShaderGlassSurface");
		expect(source).not.toContain("openDelay={0}");
		expect(source).not.toContain("tabindex={activity.output");
		expect(source).not.toContain("preview_props");
	});
});
