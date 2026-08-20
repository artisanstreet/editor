import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { thread_display_title } from "../../modules/frontend/src/lib/threads/title";

const thread = (input: { readonly summary_title?: string; readonly title_locked?: boolean }) => ({
	title: "Latest user message",
	title_locked: input.title_locked ?? false,
	...(input.summary_title === undefined ? {} : { summary_title: input.summary_title }),
});

describe("thread title mode", () => {
	it("defaults the shared reader state and Settings toggle to summaries", () => {
		const title_store = readFileSync("modules/frontend/src/lib/threads/title.ts", "utf8");
		const settings = readFileSync(
			"modules/frontend/src/routes/components/settings/thread-titles.svelte",
			"utf8",
		);

		expect(title_store).toContain("writable<ThreadTitleMode>(DefaultThreadTitleMode)");
		expect(settings).toContain('title="Summary titles"');
		expect(settings).toContain('enabled ? "summary" : "latest_message"');
	});

	it("prefers the harness summary in summary mode and falls back without one", () => {
		expect(
			thread_display_title(thread({ summary_title: "Generated summary" }), "summary"),
		).toBe("Generated summary");
		expect(thread_display_title(thread({}), "summary")).toBe("Latest user message");
	});

	it("shows the latest message in latest-message mode even when a summary exists", () => {
		expect(
			thread_display_title(thread({ summary_title: "Generated summary" }), "latest_message"),
		).toBe("Latest user message");
	});

	it("lets a manual rename outrank the summary in every mode", () => {
		expect(
			thread_display_title(
				thread({ summary_title: "Generated summary", title_locked: true }),
				"summary",
			),
		).toBe("Latest user message");
	});
});
