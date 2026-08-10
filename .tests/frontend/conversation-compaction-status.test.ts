import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const status = readFileSync(
	resolve(workspace, "modules/frontend/src/routes/components/conversation-status.svelte"),
	"utf8",
);

describe("conversation compaction status", () => {
	it("uses transcript typography and a stable compaction mark", () => {
		expect(status).toContain('item.type === "compaction" || size === "base"');
		expect(status).toContain(
			'import ArrowsMinimize from "@tabler/icons-svelte/icons/arrows-minimize"',
		);
		expect(status).toContain('<ArrowsMinimize class="size-4 shrink-0" aria-hidden="true" />');
	});

	it("shimmers the verb only while compaction is active", () => {
		const compaction = status.slice(
			status.indexOf('{:else if item.type === "compaction"}'),
			status.indexOf('{:else if item.type === "native_event"}'),
		);

		expect(compaction).toMatch(
			/\{#if item\.state === "started"\}[\s\S]*?<ShimmerText[\s\S]*?Compacting[\s\S]*?\{:else if item\.state === "failed"\}[\s\S]*?<span class="text-destructive">Compaction failed<\/span>[\s\S]*?\{:else\}[\s\S]*?<span>Compacted<\/span>/u,
		);
		expect(compaction.match(/<ShimmerText/gu)).toHaveLength(1);
	});
});
