import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ConversationItem } from "@artisan/protocol";
import { Effect, Layer, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	ThreadScrollMemory,
	ThreadScrollMemoryLive,
	conversation_content_stamp,
	thread_scroll_position_is_current,
} from "../../modules/frontend/src/lib/conversation/scroll-memory";

const WithMemory = <A>(program: (memory: typeof ThreadScrollMemory.Service) => Effect.Effect<A>) =>
	Effect.runPromise(
		Effect.scoped(
			Effect.gen(function* () {
				const services = yield* Layer.build(ThreadScrollMemoryLive);
				return yield* Effect.gen(function* () {
					return yield* program(yield* ThreadScrollMemory);
				}).pipe(Effect.provide(services));
			}),
		),
	);

const base = {
	created_at: "2026-08-15T00:00:00.000Z",
	lifecycle: "completed",
	ordinal: 1,
	references: [],
	revision: 0,
	run_id: "run_1",
	source_refs: [],
	turn_id: "turn:user:steer",
	updated_at: "2026-08-15T00:00:00.000Z",
};

const item = (value: unknown) => Schema.decodeUnknownSync(ConversationItem)(value);

describe("thread scroll memory", () => {
	it("recalls the position a thread was left at", async () => {
		const recalled = await WithMemory((memory) =>
			Effect.gen(function* () {
				yield* memory.Remember("thread_a", { content_stamp: "7:7:420", scroll_top: 420 });
				return yield* memory.Recall("thread_a");
			}),
		);

		expect(recalled).toEqual({ content_stamp: "7:7:420", scroll_top: 420 });
	});

	/** The route may carry a thread's bare historical alias; both name one thread. */
	it("recalls through the thread's route identity", async () => {
		const recalled = await WithMemory((memory) =>
			Effect.gen(function* () {
				yield* memory.Remember("thread_b", { content_stamp: "2:2:96", scroll_top: 96 });
				return yield* memory.Recall("b");
			}),
		);

		expect(recalled?.scroll_top).toBe(96);
	});

	it("forgets the least recently remembered thread once the working set is full", async () => {
		const result = await WithMemory((memory) =>
			Effect.gen(function* () {
				for (const ordinal of [1, 2, 3, 4, 5, 6, 7]) {
					yield* memory.Remember(`thread_${ordinal}`, {
						content_stamp: String(ordinal),
						scroll_top: ordinal,
					});
				}
				return {
					newest: yield* memory.Recall("thread_7"),
					oldest: yield* memory.Recall("thread_1"),
					survivor: yield* memory.Recall("thread_2"),
				};
			}),
		);

		expect(result.oldest).toBeUndefined();
		expect(result.survivor?.scroll_top).toBe(2);
		expect(result.newest?.scroll_top).toBe(7);
	});

	/**
	 * The whole point of stamping the content: a thread that sat still keeps
	 * the reader's place, and one that moved on while they were away sends them
	 * to the latest instead of stranding them mid-history under new content.
	 */
	it("holds a position only while the transcript has not moved on", () => {
		const position = { content_stamp: "3:3:300", scroll_top: 300 };

		expect(thread_scroll_position_is_current(position, "3:3:300")).toBe(true);
		expect(thread_scroll_position_is_current(position, "4:4:340")).toBe(false);
		expect(thread_scroll_position_is_current(undefined, "3:3:300")).toBe(false);
	});

	/**
	 * The stamp answers "has anything the reader can read arrived", not "has
	 * anything at all happened". The patch sequence moves when a run settles its
	 * lifecycle behind a reader who had already read everything, and keying the
	 * position to it forgot their place exactly then.
	 */
	it("moves the stamp for content only, never for lifecycle bookkeeping", () => {
		const session = item({
			...base,
			id: "session_1",
			lifecycle: "streaming",
			ordinal: 1,
			started_at: base.created_at,
			status: "active",
			title: "Working",
			type: "work_session",
		});
		const reply = item({
			...base,
			id: "assistant_1",
			lifecycle: "streaming",
			ordinal: 2,
			text: "The first half of the answer",
			type: "assistant_message",
		});
		const stamp = conversation_content_stamp([session, reply]);

		/** A run settling is not content: the reader was looking at all of it. */
		const settled = item({
			...base,
			ended_at: base.updated_at,
			id: "session_1",
			lifecycle: "completed",
			ordinal: 1,
			started_at: base.created_at,
			status: "completed",
			title: "Worked",
			type: "work_session",
		});
		const closed_reply = item({
			...base,
			id: "assistant_1",
			lifecycle: "completed",
			ordinal: 2,
			text: "The first half of the answer",
			type: "assistant_message",
		});
		expect(conversation_content_stamp([settled, closed_reply])).toBe(stamp);

		/** Streamed text growing inside the same item is content. */
		const grown = item({
			...base,
			id: "assistant_1",
			lifecycle: "streaming",
			ordinal: 2,
			text: "The first half of the answer, and now the second",
			type: "assistant_message",
		});
		expect(conversation_content_stamp([session, grown])).not.toBe(stamp);

		/** A new item is content, whoever sent it. */
		const steer = item({
			...base,
			id: "user_1",
			ordinal: 3,
			text: "Also cover the edge case",
			type: "user_message",
		});
		expect(conversation_content_stamp([session, reply, steer])).not.toBe(stamp);

		/** A command's captured output growing beneath its activity is content. */
		const running = (output: string) =>
			item({
				...base,
				id: "activity_1",
				kind: "tool_activity",
				label: "Run the tests",
				ordinal: 3,
				output,
				status: "active",
				type: "activity",
			});
		expect(conversation_content_stamp([session, reply, running("1 passed")])).not.toBe(
			conversation_content_stamp([session, reply, running("1 passed\n2 passed")]),
		);
	});

	/**
	 * The mount order that broke restoring: the viewport binds while the view
	 * state is still undefined, the transcript renders empty and grows, and the
	 * follow pin drags through that growth to the bottom. Positioning must wait
	 * for the view state, and nothing the viewport does before the reader has
	 * been placed may overwrite what the memory holds.
	 */
	it("places the reader only after the transcript renders, and remembers only after placing", () => {
		const workspace = readFileSync(
			resolve(
				import.meta.dirname,
				"../..",
				"modules/frontend/src/routes/components/thread-workspace.svelte",
			),
			"utf8",
		);

		expect(workspace).toContain(
			"const PositionLoadedThread = (view_state: ConversationViewState | undefined) =>",
		);
		expect(workspace).toContain("if (view_state === undefined) return;");
		expect(workspace).toContain("if (viewport === null || positioned) return;");
		expect(workspace).toContain("yield* PositionLoadedThread(conversation_view_state);");
		/** The memory gate inside the scroll handler's recorder. */
		expect(workspace).toContain("if (!positioned) return;");
	});
});
