import { describe, expect, it } from "vitest";
import type { ConversationItem } from "@artisan/protocol";

import {
	active_work_label_for,
	artisan_thinking_words,
	background_work_label_for,
	conversation_activity_is_live,
	conversation_background_agent_names,
	conversation_has_live_activity,
	conversation_reply_is_live,
	conversation_waiting_for_activity,
	thinking_word_at,
	thinking_word_for,
	waiting_label_for,
	work_session_is_settled,
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

	it("lets newer model text relieve a live activity wait", () => {
		const live_activity = { ...activity("activity-2", "terminal", "active"), ordinal: 2 };
		const later_reasoning = {
			...reasoning("streaming", "Still waiting, nothing new."),
			ordinal: 3,
		};
		const later_commentary = {
			...message("completed", "The command is still running."),
			ordinal: 4,
			phase: "commentary" as const,
		};

		expect(conversation_waiting_for_activity([live_activity])).toBe(true);
		expect(conversation_waiting_for_activity([live_activity, later_reasoning])).toBe(false);
		expect(conversation_waiting_for_activity([live_activity, later_commentary])).toBe(false);
	});

	it("returns to Waiting when a newer external operation starts", () => {
		const first_activity = { ...activity("activity-1", "terminal", "active"), ordinal: 1 };
		const model_text = { ...reasoning("completed"), ordinal: 2 };
		const next_activity = { ...activity("activity-3", "tool", "active"), ordinal: 3 };

		expect(conversation_waiting_for_activity([first_activity, model_text, next_activity])).toBe(
			true,
		);
	});

	it("uses durable ordinals and ignores empty text when resolving the latest phase", () => {
		const earlier_text = { ...reasoning("completed"), ordinal: 1 };
		const live_activity = { ...activity("activity-2", "terminal", "active"), ordinal: 2 };
		const empty_message = { ...message("streaming", "  \n "), ordinal: 3 };

		expect(
			conversation_waiting_for_activity([empty_message, live_activity, earlier_text]),
		).toBe(true);
		expect(
			conversation_waiting_for_activity([
				{ ...reasoning("streaming", "Fresh model text"), ordinal: 4 },
				live_activity,
			]),
		).toBe(false);
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

	/**
	 * Claude publishes nothing at all while it compacts — the boundary arrives
	 * afterwards carrying the duration it already spent — so a two-minute
	 * compaction under a waiting line is indistinguishable from a dead run.
	 */
	it("names compaction rather than the provider when the window must be rewritten", () => {
		expect(
			active_work_label_for({
				awaiting_compaction: true,
				engine_name: "Claude",
				provider_responded: false,
				seed: "work:run:run_compacting",
				waiting_for_activity: false,
			}),
		).toBe("Compacting the conversation…");
	});

	/** Once the model is answering, the wait it describes is over. */
	it("does not claim compaction after the provider has responded", () => {
		const session_id = "work:run:run_compacting_done";

		expect(
			active_work_label_for({
				awaiting_compaction: true,
				engine_name: "Claude",
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

describe("the wait on backgrounded delegated work", () => {
	const delegation = (
		id: string,
		display_name: string,
		lifecycle: Extract<ConversationItem, { type: "activity" }>["lifecycle"],
		ordinal: number,
	): Extract<ConversationItem, { type: "activity" }> => ({
		...activity(id, "subagent", lifecycle),
		ordinal,
		subagent: { agent_id: `agent-${display_name.toLowerCase()}`, display_name },
	});

	it("names live workers the model has already spoken past", () => {
		const items = [
			delegation("activity-1", "Maja", "waiting", 1),
			{ ...message("completed", "I'll synthesize once both land."), ordinal: 2 },
		];

		expect(conversation_background_agent_names(items)).toEqual(["Maja"]);
	});

	it("releases the wait as each worker settles and keeps launch order", () => {
		const both = [
			delegation("activity-1", "Maja", "waiting", 1),
			delegation("activity-2", "Ada", "active", 2),
			{ ...message("completed"), ordinal: 3 },
		];
		const one_done = [
			delegation("activity-1", "Maja", "completed", 1),
			delegation("activity-2", "Ada", "active", 2),
			{ ...message("completed"), ordinal: 3 },
		];

		expect(conversation_background_agent_names(both)).toEqual(["Maja", "Ada"]);
		expect(conversation_background_agent_names(one_done)).toEqual(["Ada"]);
	});

	/**
	 * A live delegation newer than the model's words is an ordinary foreground
	 * wait — the "Waiting" the activity wait already owns — and before any model
	 * text exists there is nothing to have spoken past.
	 */
	it("claims nothing while the delegation is the newest work or the model has not spoken", () => {
		const foreground = [
			{ ...message("completed"), ordinal: 1 },
			delegation("activity-2", "Maja", "waiting", 2),
		];

		expect(conversation_background_agent_names(foreground)).toEqual([]);
		expect(
			conversation_background_agent_names([delegation("activity-1", "Maja", "waiting", 1)]),
		).toEqual([]);
		expect(conversation_waiting_for_activity(foreground)).toBe(true);
	});

	it("reads one or two names as a sentence and a crowd as a count", () => {
		expect(background_work_label_for([])).toBeUndefined();
		expect(background_work_label_for(["Maja"])).toBe("Waiting for Maja to finish…");
		expect(background_work_label_for(["Maja", "Ada"])).toBe(
			"Waiting for Maja and Ada to finish…",
		);
		expect(background_work_label_for(["Maja", "Ada", "Faye"])).toBe(
			"Waiting for 3 background agents…",
		);
	});

	it("replaces only the thinking verb, never a foreground wait", () => {
		const session_id = "work:run:run_background";
		const label = (input: {
			background_agent_names: ReadonlyArray<string>;
			waiting_for_activity: boolean;
		}) =>
			active_work_label_for({
				engine_name: "Claude",
				provider_responded: true,
				seed: session_id,
				...input,
			});

		expect(label({ background_agent_names: ["Maja"], waiting_for_activity: false })).toBe(
			"Waiting for Maja to finish…",
		);
		expect(label({ background_agent_names: ["Maja"], waiting_for_activity: true })).toBe(
			"Waiting",
		);
		expect(label({ background_agent_names: [], waiting_for_activity: false })).toBe(
			thinking_word_for(session_id),
		);
	});
});

describe("when a session's header may claim a duration", () => {
	const updated_at = "2026-07-27T10:00:05.000Z";

	/**
	 * The send gap and the live run: a session whose own status is still live
	 * never settles. Settling in the gap flashed "Thought for 0s" over what is
	 * really the wait for the provider.
	 */
	it("keeps waiting while the session's own status is live", () => {
		for (const status of ["pending", "streaming", "active", "waiting"] as const) {
			expect(
				work_session_settlement({ ended_at: undefined, status, updated_at }),
			).toBeUndefined();
		}
	});

	/** Every terminal status settles, whatever the outcome it names. */
	it("settles on each terminal status", () => {
		for (const status of ["completed", "failed", "interrupted", "cancelled"] as const) {
			expect(work_session_settlement({ ended_at: undefined, status, updated_at })).toEqual({
				ended_at: updated_at,
			});
		}
	});

	it("always honors a genuine terminal end", () => {
		const ended_at = "2026-07-27T10:00:09.000Z";

		expect(work_session_settlement({ ended_at, status: "active", updated_at })).toEqual({
			ended_at,
		});
	});

	/**
	 * The defect this shape exists to make unrepresentable: a finished run whose
	 * completion reached the transcript could leave the header thinking, because
	 * settlement consulted a work item fetched on its own transport. There is no
	 * longer an input that could disagree — a settled transcript is a settled
	 * header, and the only way to keep waiting is for the session itself to say
	 * it is still running.
	 */
	it("cannot show a live header for a session the transcript has settled", () => {
		for (const status of ["completed", "failed", "interrupted", "cancelled"] as const) {
			expect(
				work_session_settlement({
					ended_at: "2026-07-27T10:00:09.000Z",
					status,
					updated_at,
				}),
			).toBeDefined();
		}
	});
});

describe("session liveness", () => {
	it("agrees with settlement on every lifecycle", () => {
		for (const status of ["pending", "streaming", "active", "waiting"] as const) {
			expect(work_session_is_settled(status)).toBe(false);
		}
		for (const status of ["completed", "failed", "interrupted", "cancelled"] as const) {
			expect(work_session_is_settled(status)).toBe(true);
		}
	});
});
