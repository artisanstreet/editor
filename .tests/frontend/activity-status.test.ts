import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	active_work_label_for,
	artisan_thinking_words,
	conversation_activity_is_live,
	conversation_has_live_activity,
	conversation_reply_is_live,
	thinking_word_at,
	thinking_word_for,
	waiting_label_for,
	work_session_run_authority,
	work_session_settlement,
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
	text = "Reading through the failure",
): Extract<ConversationItem, { type: "assistant_message" }> => ({
	created_at: "2026-07-27T10:00:00.000Z",
	id: "message-1",
	lifecycle,
	ordinal: 9,
	phase: "unspecified",
	references: [],
	revision: 0,
	source_refs: [],
	text,
	turn_id: "turn-1",
	type: "assistant_message",
	updated_at: "2026-07-27T10:00:00.000Z",
});

const reasoning = (
	lifecycle: Extract<ConversationItem, { type: "reasoning_summary" }>["lifecycle"],
	text = "Weighing the two layouts",
): Extract<ConversationItem, { type: "reasoning_summary" }> => ({
	created_at: "2026-07-27T10:00:00.000Z",
	id: "reasoning-1",
	lifecycle,
	ordinal: 8,
	references: [],
	revision: 0,
	source_refs: [],
	text,
	turn_id: "turn-1",
	type: "reasoning_summary",
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

	it("keeps a visibility generation stable and advances on the next generation", () => {
		const session = "work:run:visibility-epochs";
		const initial = thinking_word_for(session);
		const reappeared = thinking_word_for(session, 1);

		expect(thinking_word_for(session)).toBe(initial);
		expect(thinking_word_for(session, 1)).toBe(reappeared);
		expect(reappeared).not.toBe(initial);
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

	it("yields the status line to a streamed reply and reclaims a settled trace", () => {
		expect(conversation_reply_is_live([message("streaming")])).toBe(true);
		expect(
			conversation_reply_is_live([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "completed"),
				message("completed"),
			]),
		).toBe(false);
	});

	/**
	 * The gaps between a chain's calls are not quiet stretches. Treating a
	 * running tool as the status blinks the line out and in on every gap.
	 */
	it("holds the status line across a running tool chain", () => {
		expect(
			conversation_reply_is_live([
				activity("activity-1", "file.read", "completed"),
				activity("activity-2", "test.run", "active"),
			]),
		).toBe(false);
	});

	/**
	 * Reasoning is the thinking the word already stands for, and it opens and
	 * closes repeatedly inside one chain. Treating it as the status made the line
	 * blink for the length of the run and vanish across long private stretches.
	 */
	it("holds the status line while reasoning streams", () => {
		expect(conversation_reply_is_live([reasoning("streaming")])).toBe(false);
		expect(
			conversation_reply_is_live([
				reasoning("streaming"),
				activity("activity-1", "file.read", "active"),
			]),
		).toBe(false);
	});

	/**
	 * A streamed item exists from its first delta. An item with no character yet
	 * silenced the status line while rendering nothing, so a turn that stayed
	 * quiet for minutes looked frozen rather than working.
	 */
	it("never yields to a reply that has nothing on screen", () => {
		expect(conversation_reply_is_live([message("streaming", "")])).toBe(false);
		expect(conversation_reply_is_live([message("streaming", "   \n ")])).toBe(false);
		expect(conversation_reply_is_live([message("streaming", "A")])).toBe(true);
	});

	it("never lets a failed command with a dangling lifecycle read as running", () => {
		const ghost = {
			...activity("activity-1", "terminal", "active"),
			status: "failed" as const,
		};

		expect(conversation_activity_is_live(ghost)).toBe(false);
	});

	it("follows canonical activity start and terminal events without polling", () => {
		expect(conversation_has_live_activity([activity("activity-1", "terminal", "active")])).toBe(
			true,
		);
		expect(conversation_has_live_activity([activity("activity-1", "tool", "active")])).toBe(
			true,
		);
		expect(
			conversation_has_live_activity([activity("activity-1", "terminal", "completed")]),
		).toBe(false);
		expect(conversation_has_live_activity([reasoning("streaming")])).toBe(false);
	});
});

describe("the wait before an engine answers", () => {
	it("names the engine the request is out to", () => {
		expect(waiting_label_for("Claude")).toBe("Waiting for Claude to respond…");
		expect(waiting_label_for("Codex")).toBe("Waiting for Codex to respond…");
	});

	/** An engine nobody can name would read as "Waiting for Other to respond". */
	it("leaves unattributed work to its thinking word", () => {
		expect(waiting_label_for(undefined)).toBeUndefined();
	});

	/** A thinking verb never doubles as the wait: they answer different questions. */
	it("never borrows a thinking word", () => {
		expect(artisan_thinking_words).not.toContain(waiting_label_for("Claude"));
	});

	it("switches to the session's thinking verb once the provider starts the turn", () => {
		const session_id = "work:run:run_responded";

		expect(
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: false,
				seed: session_id,
				waiting_for_activity: false,
			}),
		).toBe("Waiting for Codex to respond…");
		expect(
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: true,
				seed: session_id,
				waiting_for_activity: false,
			}),
		).toBe(thinking_word_for(session_id));
	});

	it("shows Waiting only while a provider-started activity remains live", () => {
		const session_id = "work:run:run_activity_wait";
		const label = (waiting_for_activity: boolean) =>
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: true,
				seed: session_id,
				waiting_for_activity,
			});

		expect(label(false)).toBe(thinking_word_for(session_id));
		expect(label(true)).toBe("Waiting");
		expect(label(false)).toBe(thinking_word_for(session_id));
	});

	it("lets an activity start establish the wait before other provider detail arrives", () => {
		expect(
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: false,
				seed: "work:run:run_not_responded",
				waiting_for_activity: true,
			}),
		).toBe("Waiting");
	});

	it("applies visibility generations only after the waiting phase becomes thinking", () => {
		const session_id = "work:run:run_visibility_epoch";

		expect(
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: false,
				seed: session_id,
				thinking_visibility_generation: 1,
				waiting_for_activity: false,
			}),
		).toBe("Waiting for Codex to respond…");
		expect(
			active_work_label_for({
				engine_name: "Codex",
				provider_responded: true,
				seed: session_id,
				thinking_visibility_generation: 1,
				waiting_for_activity: false,
			}),
		).toBe(thinking_word_for(session_id, 1));
	});
});

describe("when a session's header may claim a duration", () => {
	const updated_at = "2026-07-27T10:00:05.000Z";

	/**
	 * The send gap and the live run: while the durable authority holds the run,
	 * nothing may settle. Settling in the gap flashed "Thought for 0s" over
	 * what is really the wait for the provider.
	 */
	it("keeps waiting while the authority holds the run", () => {
		for (const run_authority of ["pending", "active"] as const) {
			for (const provider_responded of [false, true]) {
				expect(
					work_session_settlement({
						ended_at: undefined,
						provider_responded,
						run_authority,
						updated_at,
					}),
				).toBeUndefined();
			}
		}
	});

	/** A run that died after responding still deserves its ordinary header. */
	it("settles an orphaned responded session on its last change", () => {
		expect(
			work_session_settlement({
				ended_at: undefined,
				provider_responded: true,
				run_authority: "settled",
				updated_at,
			}),
		).toEqual({ ended_at: updated_at, presumed_failed: false });
	});

	/**
	 * A run that died before its first byte, with its terminal event lost, must
	 * not wait forever: the response never came, so the header is a failure.
	 */
	it("settles a run that died unanswered as a presumed failure", () => {
		expect(
			work_session_settlement({
				ended_at: undefined,
				provider_responded: false,
				run_authority: "settled",
				updated_at,
			}),
		).toEqual({ ended_at: updated_at, presumed_failed: true });
	});

	it("always honors a genuine terminal end", () => {
		const ended_at = "2026-07-27T10:00:09.000Z";

		expect(
			work_session_settlement({
				ended_at,
				provider_responded: false,
				run_authority: "active",
				updated_at,
			}),
		).toEqual({ ended_at, presumed_failed: false });
	});
});

describe("durable run authority", () => {
	it("maps the owning work item's status onto the session", () => {
		const owning = (
			status: Parameters<typeof work_session_run_authority>[0]["active_run_status"],
		) =>
			work_session_run_authority({
				active_run_id: "run_current",
				active_run_status: status,
				newest_session_run_id: "run_current",
				session_run_id: "run_current",
			});

		expect(owning("queued")).toBe("pending");
		expect(owning("running")).toBe("active");
		expect(owning("waiting")).toBe("active");
		expect(owning("interrupted")).toBe("settled");
		expect(owning("completed")).toBe("settled");
		expect(owning("cancelled")).toBe("settled");
		expect(owning(undefined)).toBe("settled");
	});

	/**
	 * The work item refreshes on its own channel and may not describe a session
	 * this new yet — including the very first message of a thread, where no
	 * work item exists at all. Presuming death there would flash a failure
	 * over every freshly sent message.
	 */
	it("keeps the transcript's newest session pending while the work item catches up", () => {
		expect(
			work_session_run_authority({
				active_run_id: "run_previous",
				active_run_status: "completed",
				newest_session_run_id: "run_new",
				session_run_id: "run_new",
			}),
		).toBe("pending");
		expect(
			work_session_run_authority({
				active_run_id: undefined,
				active_run_status: undefined,
				newest_session_run_id: "run_first",
				session_run_id: "run_first",
			}),
		).toBe("pending");
	});

	it("settles history the work item no longer describes", () => {
		expect(
			work_session_run_authority({
				active_run_id: "run_current",
				active_run_status: "running",
				newest_session_run_id: "run_current",
				session_run_id: "run_old",
			}),
		).toBe("settled");
		expect(
			work_session_run_authority({
				active_run_id: "run_current",
				active_run_status: "running",
				newest_session_run_id: "run_current",
				session_run_id: undefined,
			}),
		).toBe("settled");
	});
});
