import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	artisan_thinking_words,
	conversation_activity_is_live,
	conversation_work_is_live,
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

const message = (
	lifecycle: Extract<ConversationItem, { type: "assistant_message" }>["lifecycle"],
): Extract<ConversationItem, { type: "assistant_message" }> => ({
	created_at: "2026-07-27T10:00:00.000Z",
	id: "message-1",
	lifecycle,
	ordinal: 9,
	phase: "unspecified",
	references: [],
	revision: 0,
	source_refs: [],
	text: "Reading through the failure",
	turn_id: "turn-1",
	type: "assistant_message",
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

	it("yields the status line to live work and reclaims a settled trace", () => {
		expect(
			conversation_work_is_live([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "active"),
			]),
		).toBe(true);
		expect(conversation_work_is_live([message("streaming")])).toBe(true);
		expect(
			conversation_work_is_live([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "completed"),
				message("completed"),
			]),
		).toBe(false);
	});

	it("never lets a failed command with a dangling lifecycle read as running", () => {
		const ghost = {
			...activity("activity-1", "terminal", "active"),
			status: "failed" as const,
		};

		expect(conversation_activity_is_live(ghost)).toBe(false);
		expect(conversation_work_is_live([ghost])).toBe(false);
	});
});
