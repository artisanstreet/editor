import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const source = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/new-thread-route.svelte",
	),
	"utf8",
);

describe("new thread layout", () => {
	it("centres the introduction on the composer's column, clear of the card", () => {
		expect(source).toContain(
			'class="prose-column-frame absolute inset-0 flex flex-col items-center justify-center pb-44"',
		);
		expect(source).toContain(
			'class="prose-column flex w-full max-w-(--prose-width) flex-col gap-3 text-center"',
		);
	});

	it("lays out the undivided panel in place of the sentence", () => {
		expect(source).toContain(
			'class="grid aspect-3/2 w-4/5 min-h-0 grid-cols-[minmax(0,2fr)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] self-center"',
		);
		/**
		 * No rule between or inside the panes. The panel's own class above is
		 * already outline-free; the recovery banner keeps its border and is not
		 * part of this surface.
		 */
		expect(source).not.toContain("<Separator");
		expect(source).not.toContain("NewThreadSentenceWords");
		expect(source).not.toContain("Nothing is attached yet");
	});

	it("scrolls recent threads in the left pane without a redundant new-thread action", () => {
		/** Freshest first, and the whole list — not the rail's settled-only subset. */
		expect(source).toContain("SortRecentThreads(threads)");
		expect(source).toContain("ThreadRoutePathFor(thread)");
		expect(source).toContain("FormatRecentThreadTime(ThreadLastMessageAt(thread), now_ms)");
		/** A recent thread is identified by its harness, not its model's provider lab. */
		expect(source).toContain("EngineMarkFor(thread.engine_id)");
		expect(source).not.toContain("UsageSlicePresentationFor");
		/** The list owns the pane and scrolls within it. */
		expect(source).toContain(
			'class="docs-scroll-fade relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto p-1"',
		);
		expect(source).not.toContain('trigger_label="New thread"');
		expect(source).not.toContain("<ProjectSelector");
	});

	it("charts a year of daily token spend in the right pane", () => {
		expect(source).toContain("<VerticalCalendarActivityGrid {activities} />");
		expect(source).toContain("client.GetSurfaceUsageDaily({ day_count: usage_day_count })");
		/** A fresh install still draws the year rather than collapsing the pane. */
		expect(source).toContain("empty_usage_days()");
	});
});
