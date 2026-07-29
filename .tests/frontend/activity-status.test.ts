import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	artisan_thinking_words,
	latest_active_activity_label,
	thinking_word_at,
	thinking_word_for,
} from "../../modules/frontend/src/lib/conversation/activity-status";

const activity = (
	id: string,
	kind: string,
	lifecycle: Extract<ConversationItem, { type: "activity" }>["lifecycle"],
): Extract<ConversationItem, { type: "activity" }> => ({
	created_at: "2026-07-27T10:00:00.000Z",
	id,
	kind,
	label: "Provider activity",
	lifecycle,
	ordinal: Number(id.slice(-1)),
	references: [],
	revision: 0,
	source_refs: [],
	status: lifecycle,
	turn_id: "turn-1",
	type: "activity",
	updated_at: "2026-07-27T10:00:00.000Z",
});

describe("per-session thinking word", () => {
	it("keeps one word for a session and varies it across sessions", () => {
		const session = "work:run:run_1";

		expect(thinking_word_for(session)).toBe(thinking_word_for(session));
		expect(artisan_thinking_words).toContain(thinking_word_for(session));

		const chosen = new Set(
			Array.from({ length: 200 }, (_, index) => thinking_word_for(`work:run:run_${index}`)),
		);
		/** A per-session choice must not collapse to a single constant word. */
		expect(chosen.size).toBeGreaterThan(1);
		for (const word of chosen) expect(artisan_thinking_words).toContain(word);
	});
});

describe("Artisan thinking vocabulary", () => {
	it("is curated, unique, and excludes flat legacy labels", () => {
		expect(artisan_thinking_words.length).toBeGreaterThan(5);
		expect(new Set(artisan_thinking_words).size).toBe(artisan_thinking_words.length);
		expect(artisan_thinking_words).not.toContain("Thinking");
		expect(artisan_thinking_words).not.toContain("Working");
	});

	it("rotates through the data vocabulary", () => {
		expect(thinking_word_at(0)).toBe(artisan_thinking_words[0]);
		expect(thinking_word_at(artisan_thinking_words.length)).toBe(artisan_thinking_words[0]);
		expect(thinking_word_at(artisan_thinking_words.length + 1)).toBe(artisan_thinking_words[1]);
	});

	it("prefers the latest observable engine activity over a whimsical verb", () => {
		expect(
			latest_active_activity_label([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "active"),
			]),
		).toBe("Running tests");
		expect(
			latest_active_activity_label([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "completed"),
			]),
		).toBeUndefined();
	});
});
