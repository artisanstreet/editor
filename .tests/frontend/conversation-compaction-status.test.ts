import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const status = readFileSync(
	resolve(workspace, "modules/frontend/src/routes/components/conversation-status.svelte"),
	"utf8",
);

describe("conversation compaction status", () => {
	it("renders compaction as a full-width chapter divider", () => {
		expect(status).toContain('import { Separator } from "$lib/components/ui/separator"');
		expect(status).toContain(
			'class="flex w-full min-w-0 flex-row items-center gap-4 py-0.5 text-base text-muted-foreground"',
		);
		expect(
			status.match(/<Separator class="min-w-0 flex-1" aria-hidden="true" \/>/gu),
		).toHaveLength(2);
		expect(status).not.toContain("ArrowsMinimize");
	});

	it("keeps one label mounted and stops its shimmer after compaction", () => {
		const compaction = status.slice(status.indexOf('{:else if item.type === "compaction"}'));

		expect(compaction.match(/<ShimmerText/gu)).toHaveLength(1);
		expect(compaction).toContain('active={item.state === "started"}');
		expect(compaction).toContain('? "shrink-0 text-destructive"');
		expect(compaction).toContain('? "Compacting"');
		expect(compaction).toContain('? "Compaction failed"');
		expect(compaction).toContain(': "Compacted"');
	});
});
